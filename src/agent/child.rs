//! A subagent's run: a conversation of its own, kept beside its parent's, run under the run whose
//! `task` call starts it. It answers with its last reply.

use std::path::PathBuf;

use crate::agent::kinds::{self, Kind};
use crate::agent::{Run, RunCtx, run};
use crate::conversation::message::{Assistant, Message, Part, Usage};
use crate::live::event::{AgentEvent, Ended, Outcome};
use crate::session::entry::Parent;
use crate::session::transcript::Transcript;

/// How a subagent's run went.
pub struct Done {
    pub ended: Ended,
    /// Its last reply's words, if it said any.
    pub answer: Option<String>,
    /// Why its last reply failed, if it did.
    pub error: Option<String>,
    pub session: PathBuf,
    pub calls: usize,
    /// What all its requests cost.
    pub usage: Usage,
}

/// Runs `prompt` as a subagent of `kind`, started by `parent`'s call `call_id`. What it says
/// reaches the frontend wrapped in that call; it asks the user through `parent`'s prompts.
// CEILING: every call starts a fresh subagent, which knows nothing of an earlier one. Upgrade: a
// `task_id` argument that reopens its file with `Transcript::open`.
pub async fn run_child(parent: &Run<'_>, call_id: &str, kind: &Kind, prompt: String) -> Done {
    let cx = parent.cx;
    // It waits for one of its parent's places, or for the interrupt that ends the wait.
    let slot = tokio::select! {
        slot = parent.slots.acquire() => slot.ok(),
        () = parent.cancel.cancelled() => None,
    };
    let Some(_slot) = slot else {
        return Done {
            ended: Ended {
                outcome: Outcome::Aborted,
                turns: 0,
            },
            answer: None,
            error: None,
            session: PathBuf::new(),
            calls: 0,
            usage: Usage::default(),
        };
    };
    let tools = kinds::tools(kind, cx.tools, parent.depth() + 1);
    let shared = parent.shared.child(parent.cancel.clone());
    let session = parent
        .session
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut transcript = Transcript::child(
        &cx.app.paths,
        Parent {
            session,
            call: call_id.to_owned(),
            agent: kind.id.clone(),
        },
    );
    // The parent's prompt and the kind's words, the same for every subagent of this kind, so
    // the vendor can cache them.
    // CEILING: its tool list differs from the parent's, so the parent's cached prefix is not
    // reused. Upgrade: send the parent's tools and refuse the others at the gate.
    let system = format!("{}\n\n{}", cx.system, kind.prompt.trim());
    let emit = |event: AgentEvent| {
        (cx.emit)(AgentEvent::Child {
            call_id: call_id.to_owned(),
            event: Box::new(event),
        });
    };
    // The session's preset picks for it too, by its kind.
    let child = RunCtx {
        app: cx.app,
        model: cx.model,
        effort: cx.effort,
        preset: cx.preset,
        agent: Some(&kind.id),
        system: &system,
        tools: &tools,
        emit: &emit,
        shadow: cx.shadow,
    };
    let prompt = vec![Part::Text { text: prompt }];
    let ended = run::run_under(&child, &mut transcript, &shared, prompt, Some(parent)).await;
    let last = transcript.messages.iter().rev().find_map(|m| match m {
        Message::Assistant(reply) => Some(reply),
        _ => None,
    });
    Done {
        ended,
        answer: last.and_then(answer),
        error: last
            .and_then(|reply| reply.error.as_ref())
            .map(crate::api::error::headline),
        session: transcript.file().to_path_buf(),
        calls: transcript
            .messages
            .iter()
            .filter(|m| matches!(m, Message::Tool(_)))
            .count(),
        usage: transcript.usage(),
    }
}

/// What a subagent's last reply says: its words, else the summary it ended with through
/// `finish`, which a subagent that ends that way often writes instead.
fn answer(reply: &Assistant) -> Option<String> {
    let said = Part::text_of(&reply.parts);
    if !said.trim().is_empty() {
        return Some(said);
    }
    let call = reply
        .tool_calls()
        .find(|c| c.name == crate::tools::FINISH)?;
    let args: serde_json::Value = serde_json::from_str(&call.arguments).ok()?;
    let summary = args.get("summary")?.as_str()?.trim();
    (!summary.is_empty()).then(|| summary.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::{Value, json};

    use crate::agent::testing::{Harness, last_message, reply, shared, text};
    use crate::api::models::Model;
    use crate::conversation::message::Message;
    use crate::conversation::message::Part;
    use crate::live::event::{AgentEvent, Outcome};
    use crate::models::{Catalog, Source, preset};
    use crate::session::list;
    use crate::session::transcript::Transcript;

    fn task(prompt: &str) -> String {
        let args = json!({"description": "look", "prompt": prompt, "subagent_type": "explore"});
        reply("", &[("c1", "task", args)], "tool_calls")
    }

    #[tokio::test]
    async fn a_preset_picks_each_request_by_the_kind_that_sends_it() {
        let mut h = Harness::new(vec![task("find it"), text("found"), text("done")]).await;
        let named = |id: &str| Model {
            id: id.into(),
            ..crate::testing::model(true)
        };
        let catalog = Catalog {
            models: vec![named("cheap"), named("strong")],
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        let text = "[[rule]]\nwhen = { agent = [\"explore\"] }\nmodel = \"cheap\"\neffort = \"low\"\n\
                    [[rule]]\nmodel = \"strong\"";
        h.preset = Some(preset::parsed("p", text).resolve(&catalog).unwrap());
        let (ended, transcript, _) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        let sent: Vec<(&str, Option<&str>)> = requests
            .iter()
            .map(|r| (r["model"].as_str().unwrap(), r["reasoning_effort"].as_str()))
            .collect();
        assert_eq!(
            sent,
            [("strong", None), ("cheap", Some("low")), ("strong", None)]
        );
        // The prompt records the preset, so going on with the session picks the same way.
        let (_, resumed) = Transcript::open(transcript.path().unwrap()).unwrap();
        assert_eq!(resumed.state.unwrap().model, "p");
    }

    fn tool_names(request: &Value) -> Vec<String> {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_subagent_answers_in_a_file_of_its_own_with_explores_tools() {
        let h = Harness::new(vec![
            task("find the parser"),
            text("THE CHILD'S ANSWER"),
            text("done"),
        ])
        .await;
        let (ended, transcript, events) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        let child = &requests[1];
        assert_eq!(tool_names(child), ["read", "glob", "grep", "webfetch"]);
        let messages = child["messages"].as_array().unwrap();
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("You are a subagent")
        );
        // Its prompt alone: no PLAN reminder, and nothing of the parent's conversation.
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "find the parser");
        assert_eq!(last_message(&requests[2])["content"], "THE CHILD'S ANSWER");
        assert!(events.iter().any(|e| e["type"] == "child"));

        let parent = transcript.path().unwrap();
        let folder = parent.with_extension("");
        let files = crate::io::fs::list_dir(&folder).unwrap();
        assert_eq!(files.len(), 1);
        let text = crate::io::fs::read_string(&folder.join(&files[0].name))
            .unwrap()
            .unwrap();
        let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(header["parent"]["call"], "c1");
        assert_eq!(header["parent"]["agent"], "explore");
        // Lists of the project's sessions show the parent's file only.
        assert_eq!(list::list(&h.app.paths).len(), 1);
        assert_eq!(list::latest(&h.app.paths).as_deref(), Some(parent));

        // What the subagent spent is the task result's, and a reopened session counts it.
        let spent = |t: &Transcript| t.usage().cost_micros;
        let child = result_usage(&transcript).unwrap();
        assert_eq!((child.input, child.output), (10, 2));
        assert!(child.cost_micros > 0);
        let (reopened, resumed) = Transcript::open(parent).unwrap();
        assert_eq!(spent(&reopened), spent(&transcript));
        assert_eq!(resumed.spent_micros, spent(&transcript));
        assert!(spent(&transcript) > child.cost_micros);
    }

    /// The usage recorded with the transcript's task result.
    fn result_usage(transcript: &Transcript) -> Option<crate::conversation::message::Usage> {
        transcript.messages.iter().find_map(|m| match m {
            Message::Tool(result) if result.name == "task" => result.usage,
            _ => None,
        })
    }

    #[tokio::test]
    async fn an_interrupt_at_a_subagents_prompt_stops_both_runs() {
        let fetch = reply(
            "",
            &[("c1", "webfetch", json!({"url": "https://example.com"}))],
            "tool_calls",
        );
        let h = Harness::new(vec![task("fetch it"), fetch, text("never")]).await;
        let (ended, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if matches!(event.innermost(), AgentEvent::Prompt { .. }) {
                    shared.interrupt();
                }
            })
            .await;
        assert_eq!(ended.outcome, Outcome::Aborted);
        assert_eq!(h.requests().len(), 2);
        // Waited for, not dropped: what the subagent spent before the interrupt is kept.
        let kept = result_usage(&transcript).unwrap();
        assert_eq!((kept.input, kept.output), (10, 2));
    }

    #[tokio::test]
    async fn a_message_steered_meanwhile_reaches_the_parent_only() {
        let h = Harness::new(vec![
            task("look around"),
            text("child done"),
            text("parent done"),
        ])
        .await;
        let steered = AtomicBool::new(false);
        h.run(&shared("manual"), |event, shared| {
            if matches!(event, AgentEvent::Child { .. }) && !steered.swap(true, Ordering::Relaxed) {
                shared.steer(vec![Part::Text {
                    text: "STEERED".into(),
                }]);
            }
        })
        .await;
        let requests = h.requests();
        assert!(!requests[1]["messages"].to_string().contains("STEERED"));
        assert!(requests[2]["messages"].to_string().contains("STEERED"));
    }

    #[tokio::test]
    async fn a_subagent_stops_at_its_own_turn_limit_and_cannot_start_another() {
        let limit = crate::config::limits::get().subagent.max_turns.value;
        let again = json!({"description": "x", "prompt": "x", "subagent_type": "explore"});
        let mut replies = vec![task("go deeper")];
        replies
            .extend((0..limit).map(|_| reply("", &[("c9", "task", again.clone())], "tool_calls")));
        replies.push(text("done"));
        let h = Harness::new(replies).await;
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len() as u32, limit + 2);
        // The subagent was told it has no `task`, every time.
        assert!(
            last_message(&requests[2])["content"]
                .as_str()
                .unwrap()
                .contains("no tool named `task`")
        );
        let result = last_message(&requests[requests.len() - 1]);
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .contains(&format!("after {limit} requests"))
        );
    }

    #[tokio::test]
    async fn subagents_started_together_keep_to_their_places_and_answer_in_call_order() {
        let place = crate::config::limits::get().subagent.parallel.value;
        let calls: Vec<(String, Value)> = (0..place + 1)
            .map(|i| {
                let args = json!({"description": "x", "prompt": "x", "subagent_type": "explore"});
                (format!("c{i}"), args)
            })
            .collect();
        let calls: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(id, args)| (id.as_str(), "task", args.clone()))
            .collect();
        let mut replies = vec![reply("", &calls, "tool_calls")];
        replies.extend((0..place + 1).map(|_| text("same answer")));
        replies.push(text("done"));
        let h = Harness::answering(replies, std::time::Duration::from_millis(100)).await;
        let (ended, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        assert_eq!(h.most_in_flight(), place);
        let results: Vec<&str> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some(r.call_id.as_str()),
                _ => None,
            })
            .collect();
        let ids: Vec<String> = (0..place + 1).map(|i| format!("c{i}")).collect();
        assert_eq!(results, ids);
    }

    fn general(id: &str, prompt: &str) -> (String, &'static str, Value) {
        let args =
            json!({"description": "change it", "prompt": prompt, "subagent_type": "general"});
        (id.to_owned(), "task", args)
    }

    fn write(path: &str) -> String {
        reply(
            "",
            &[("w1", "write", json!({"path": path, "content": "x"}))],
            "tool_calls",
        )
    }

    /// The snapshot lines in the session file at `path`.
    fn snapshots(path: &std::path::Path) -> Vec<Value> {
        crate::io::fs::read_string(path)
            .unwrap()
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|l| l["type"] == "snapshot")
            .collect()
    }

    #[tokio::test]
    async fn a_general_subagents_edits_are_the_parent_prompts_for_undo() {
        let calls = [general("c1", "make a file"), general("c2", "make another")];
        let calls: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(i, t, a)| (i.as_str(), *t, a.clone()))
            .collect();
        let replies = vec![
            reply("", &calls, "tool_calls"),
            write("a.txt"),
            write("b.txt"),
            text("wrote it"),
            text("wrote it"),
            text("done"),
        ];
        let Some(h) = Harness::new(replies).await.with_git().await else {
            return;
        };
        let (ended, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let project = &h.app.paths.project;
        assert!(crate::io::fs::exists(&project.join("a.txt")));
        assert!(crate::io::fs::exists(&project.join("b.txt")));
        // One snapshot for the prompt, in the parent's file, however many subagents changed things.
        let parent = transcript.path().unwrap();
        let snaps = snapshots(parent);
        assert_eq!(snaps.len(), 2, "{snaps:?}");
        let folder = parent.with_extension("");
        for child in crate::io::fs::list_dir(&folder).unwrap() {
            assert!(snapshots(&folder.join(child.name)).is_empty());
        }
        let (before, after) = (snaps[1]["before"].as_str(), snaps[1]["after"].as_str());
        let restored = h.shadow.restore(before.unwrap(), after).await.unwrap();
        assert_eq!(restored.files, 2);
        assert!(!crate::io::fs::exists(&project.join("a.txt")));
    }

    #[tokio::test]
    async fn under_plan_a_general_subagent_cannot_write() {
        let calls = [general("c1", "make a file")];
        let calls: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(i, t, a)| (i.as_str(), *t, a.clone()))
            .collect();
        let h = Harness::new(vec![
            reply("", &calls, "tool_calls"),
            write("a.txt"),
            text("could not"),
            text("done"),
        ])
        .await;
        h.run(&shared("plan"), |_, _| {}).await;
        let refused = last_message(&h.requests()[2]);
        assert!(
            refused["content"].as_str().unwrap().contains("Not allowed"),
            "{refused}"
        );
        assert!(!crate::io::fs::exists(&h.app.paths.project.join("a.txt")));
    }

    /// A run whose explore subagent's webfetch asks, answered `answer`.
    async fn asked(answer: crate::live::prompt::Reply) -> (Harness, Outcome) {
        let fetch = reply(
            "",
            &[("c1", "webfetch", json!({"url": "https://example.com"}))],
            "tool_calls",
        );
        let h = Harness::new(vec![
            task("fetch it"),
            fetch,
            text("child goes on"),
            text("done"),
        ])
        .await;
        let (ended, _, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event.innermost() {
                    shared.prompts.answer(*id, answer.clone());
                }
            })
            .await;
        (h, ended.outcome)
    }

    #[tokio::test]
    async fn a_plain_no_inside_a_subagent_stops_its_parent_and_words_carry_on() {
        use crate::live::prompt::Reply;
        let (h, outcome) = asked(Reply::No { feedback: None }).await;
        assert_eq!(outcome, Outcome::Rejected);
        assert_eq!(h.requests().len(), 2);

        let why = Reply::No {
            feedback: Some("not that site".into()),
        };
        let (h, outcome) = asked(why).await;
        assert_eq!(outcome, Outcome::Done);
        assert!(
            h.requests()[2]["messages"]
                .to_string()
                .contains("not that site")
        );
        assert_eq!(last_message(&h.requests()[3])["content"], "child goes on");

        let (h, outcome) = asked(Reply::Unavailable).await;
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(h.requests().len(), 4);
    }

    #[tokio::test]
    async fn a_subagent_that_ends_with_finish_answers_with_its_summary() {
        let summary = "Typed parser.py; behaviour unchanged.";
        let finish = reply(
            "",
            &[("f1", "finish", json!({ "summary": summary }))],
            "tool_calls",
        );
        let h = Harness::new(vec![task("type it"), finish, text("done")]).await;
        let (ended, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let result = transcript.messages.iter().find_map(|m| match m {
            Message::Tool(r) if r.name == "task" => Some(r.clone()),
            _ => None,
        });
        let result = result.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, summary);
    }
}
