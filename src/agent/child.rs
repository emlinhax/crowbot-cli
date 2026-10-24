//! A subagent's run: a conversation of its own, kept beside its parent's, run under the run whose
//! `task` call starts it. It answers with its last reply.

use std::path::PathBuf;

use crate::agent::claims::{self, Holder, Table};
use crate::agent::kinds::{self, Base, Kind};
use crate::agent::roles::{self, Resolved};
use crate::agent::system_prompt;
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

impl Run<'_> {
    /// What a subagent of `kind` started by this run would run on, `asked` being the call's own
    /// pick.
    pub(crate) fn subagent(&self, kind: &Kind, asked: Option<&str>) -> Result<Resolved, String> {
        let cx = self.cx;
        let chat = roles::Chat {
            model: cx.model,
            effort: cx.effort,
        };
        roles::resolve(&kind.id, asked, cx.subagents, cx.catalog, &chat)
    }
}

/// What a subagent of `kind` on `model` is told: the parent's prompt, told the model it runs on,
/// then the kind's words, the same for every subagent of this kind on one model so the vendor can
/// cache them; or, for an agent file that replaces it, the kind's words and the environment alone.
// CEILING: its tool list differs from the parent's, so the parent's cached prefix is not reused.
// Upgrade: send the parent's tools and refuse the others at the gate.
fn system_for(cx: &RunCtx<'_>, kind: &Kind, model: &str) -> String {
    match kind.base {
        Base::Append => {
            let base = system_prompt::retarget(cx.system, &cx.model.id, model);
            format!("{base}\n\n{}", kind.prompt.trim())
        }
        Base::Replace => {
            let env = system_prompt::env(&cx.app.paths, model);
            format!("{}\n\n{env}", kind.prompt.trim())
        }
    }
}

/// Runs `prompt` as a subagent of `kind` on `on`, started by `parent`'s call `call_id`. What it
/// says reaches the frontend wrapped in that call; it asks the user through `parent`'s prompts.
// CEILING: every call starts a fresh subagent, which knows nothing of an earlier one. Upgrade: a
// `task_id` argument that reopens its file with `Transcript::open`.
pub async fn run_child(
    parent: &Run<'_>,
    call_id: &str,
    kind: &Kind,
    on: &Resolved,
    job: Job,
) -> Result<Done, String> {
    let claims = parent.claims();
    claims.register(Holder {
        call: call_id.to_owned(),
        kind: kind.id.clone(),
        job: job.description,
        model: on.model.id.clone(),
        scope: job.scope,
    })?;
    // Released however the run ends, so no file stays held by a subagent that is gone.
    let _held = Release(claims, call_id);
    // Subagents started in one batch all register before any is told who else is at work.
    tokio::task::yield_now().await;
    Ok(run_held(parent, call_id, kind, on, job.prompt).await)
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

/// What a `task` call hands its subagent.
pub struct Job {
    /// A few words for it, as the others are told.
    pub description: String,
    pub prompt: String,
    /// What it may change; empty, any file no other subagent holds.
    pub scope: Vec<String>,
}

struct Release<'a>(&'a Table, &'a str);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.release(self.1);
    }
}

async fn run_held(
    parent: &Run<'_>,
    call_id: &str,
    kind: &Kind,
    on: &Resolved,
    prompt: String,
) -> Done {
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
    let system = system_for(cx, kind, &on.model.id);
    let emit = |event: AgentEvent| {
        (cx.emit)(AgentEvent::Child {
            call_id: call_id.to_owned(),
            event: Box::new(event),
        });
    };
    let child = RunCtx {
        app: cx.app,
        model: &on.model,
        effort: on.effort.as_deref(),
        catalog: cx.catalog,
        subagents: cx.subagents,
        system: &system,
        tools: &tools,
        emit: &emit,
        shadow: cx.shadow,
    };
    // Who else is at work matters to a subagent that can change files; one that only reads is
    // spared the rules.
    let mut prompt = vec![Part::Text { text: prompt }];
    if claims::CHANGING
        .iter()
        .any(|name| tools.get(name).is_some())
    {
        let text = parent.claims().roster(call_id);
        prompt.insert(0, Part::Reminder { text });
    }
    let ended = run::run_under(
        &child,
        &mut transcript,
        &shared,
        prompt,
        Some(parent),
        Some(call_id),
    )
    .await;
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::{Value, json};

    use crate::agent::testing::{Harness, last_message, reply, shared, text};
    use crate::conversation::message::Message;
    use crate::conversation::message::Part;
    use crate::live::event::{AgentEvent, Outcome};
    use crate::session::list;
    use crate::session::transcript::Transcript;

    fn task(prompt: &str) -> String {
        let args = json!({"description": "look", "prompt": prompt, "subagent_type": "explore"});
        reply("", &[("c1", "task", args)], "tool_calls")
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

    use super::system_for;
    use crate::agent::RunCtx;
    use crate::agent::kinds::{self, Base};
    use crate::config::subagents::Role;

    fn on(model: &str) -> Role {
        Role {
            model: Some(model.into()),
            effort: None,
        }
    }

    fn task_call(id: &str, kind: &str, model: Option<&str>) -> (String, String, Value) {
        let mut args = json!({"description": kind, "prompt": format!("job for {kind}"), "subagent_type": kind});
        if let Some(model) = model {
            args["model"] = json!(model);
        }
        (id.to_owned(), "task".to_owned(), args)
    }

    fn tasks(calls: &[(String, String, Value)]) -> String {
        let calls: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(id, name, args)| (id.as_str(), name.as_str(), args.clone()))
            .collect();
        reply("", &calls, "tool_calls")
    }

    /// The model each request asked for, the parent's first.
    fn models(h: &Harness) -> Vec<String> {
        h.requests()
            .iter()
            .map(|r| r["model"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The child sessions' files beside the parent's; none when no subagent ever ran.
    fn children(transcript: &Transcript) -> Vec<std::path::PathBuf> {
        let folder = transcript.path().unwrap().with_extension("");
        crate::io::fs::list_dir(&folder)
            .unwrap_or_default()
            .into_iter()
            .map(|f| folder.join(f.name))
            .collect()
    }

    #[tokio::test]
    async fn an_explore_task_sends_the_explore_model_and_the_parent_keeps_the_chat_model() {
        let mut h = Harness::new(vec![task("find it"), text("found"), text("done")]).await;
        let explore = h.catalog.models[0].id.clone();
        h.subagents.set(Some("explore"), on(&explore));
        let (ended, transcript, _) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        assert_eq!(
            models(&h),
            [h.model.id.clone(), explore.clone(), h.model.id.clone()]
        );
        // The subagent is told the model it runs on, not the chat's.
        let result = transcript.messages.iter().find_map(|m| match m {
            Message::Tool(r) if r.name == "task" => r.details.clone(),
            _ => None,
        });
        assert_eq!(result.unwrap()["model"], explore.as_str());
    }

    #[tokio::test]
    async fn an_explore_and_a_general_task_in_one_batch_run_on_their_own_models() {
        let batch = tasks(&[
            task_call("c1", "explore", None),
            task_call("c2", "general", None),
        ]);
        let mut h = Harness::new(vec![batch, text("one"), text("two"), text("done")]).await;
        let (explore, general) = (
            h.catalog.models[0].id.clone(),
            h.catalog.models[1].id.clone(),
        );
        h.subagents.set(Some("explore"), on(&explore));
        h.subagents.set(Some("general"), on(&general));
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 4);
        // The two subagents ask in either order; each by its own tools.
        for child in &requests[1..3] {
            let want = match tool_names(child).len() {
                4 => &explore,
                _ => &general,
            };
            assert_eq!(child["model"].as_str(), Some(want.as_str()));
        }
        assert_ne!(requests[1]["model"], requests[2]["model"]);
        assert_eq!(requests[0]["model"], h.model.id.as_str());
        assert_eq!(requests[3]["model"], h.model.id.as_str());
    }

    #[tokio::test]
    async fn a_model_the_call_names_is_sent_and_an_unknown_one_is_refused_before_any_request() {
        let picked = {
            let h = Harness::new(Vec::new()).await;
            h.catalog.models[2].id.clone()
        };
        let named = tasks(&[task_call("c1", "explore", Some(&picked))]);
        let h = Harness::new(vec![named, text("found"), text("done")]).await;
        h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(models(&h)[1], picked);

        let typo = tasks(&[task_call("c1", "explore", Some("no-such-model"))]);
        let h = Harness::new(vec![typo, text("done")]).await;
        let (_, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        // The parent's two requests only: nothing went to a subagent.
        assert_eq!(models(&h), [h.model.id.clone(), h.model.id.clone()]);
        let refused = transcript.messages.iter().find_map(|m| match m {
            Message::Tool(r) if r.name == "task" => Some(r.clone()),
            _ => None,
        });
        let refused = refused.unwrap();
        assert!(
            refused.is_error && refused.content.contains("no-such-model"),
            "{refused:?}"
        );
        assert!(children(&transcript).is_empty());
    }

    #[tokio::test]
    async fn a_new_default_leaves_a_finished_tasks_recorded_model_alone() {
        let mut h = Harness::new(vec![
            task("first"),
            text("one"),
            text("done"),
            task("second"),
            text("two"),
            text("done"),
        ])
        .await;
        let (before, after) = (
            h.catalog.models[0].id.clone(),
            h.catalog.models[1].id.clone(),
        );
        h.subagents.set(Some("explore"), on(&before));
        let (_, first, _) = h.run(&shared("plan"), |_, _| {}).await;
        h.subagents.set(Some("explore"), on(&after));
        h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(models(&h)[4], after);

        let files = children(&first);
        assert_eq!(files.len(), 1);
        let (child, resumed) = Transcript::open(&files[0]).unwrap();
        assert_eq!(resumed.state.unwrap().model, before);
        let replied = child.messages.iter().find_map(|m| match m {
            Message::Assistant(reply) => Some(reply.model.clone()),
            _ => None,
        });
        assert_eq!(replied.as_deref(), Some(before.as_str()));
        let (parent, _) = Transcript::open(first.path().unwrap()).unwrap();
        let details = parent.messages.iter().find_map(|m| match m {
            Message::Tool(r) if r.name == "task" => r.details.clone(),
            _ => None,
        });
        assert_eq!(details.unwrap()["model"], before.as_str());
    }

    #[tokio::test]
    async fn an_agent_file_that_replaces_the_prompt_keeps_only_its_words_and_the_environment() {
        let h = Harness::new(Vec::new()).await;
        let emit = |_: AgentEvent| {};
        let registry = crate::tools::Registry::builtin(None);
        let parent = format!(
            "THE SESSION PROMPT\n\n{}",
            crate::agent::system_prompt::env(&h.app.paths, &h.model.id)
        );
        let cx = RunCtx {
            app: &h.app,
            model: &h.model,
            effort: None,
            catalog: &h.catalog,
            subagents: &h.subagents,
            system: &parent,
            tools: &registry,
            emit: &emit,
            shadow: &h.shadow,
        };
        let mut kind = kinds::get("explore").unwrap().clone();
        kind.prompt = "ONLY THIS".into();

        let appended = system_for(&cx, &kind, "cheap");
        assert!(appended.starts_with("THE SESSION PROMPT"), "{appended}");
        assert!(appended.ends_with("ONLY THIS") && appended.contains("Model: cheap"));

        kind.base = Base::Replace;
        let replaced = system_for(&cx, &kind, "cheap");
        assert!(replaced.starts_with("ONLY THIS"), "{replaced}");
        assert!(!replaced.contains("THE SESSION PROMPT"), "{replaced}");
        assert!(
            replaced.contains("Model: cheap") && replaced.contains("<env>"),
            "{replaced}"
        );
    }

    fn scoped(id: &str, kind: &str, scope: &[&str]) -> (String, String, Value) {
        let (id, name, mut args) = task_call(id, kind, None);
        args["scope"] = json!(scope);
        (id, name, args)
    }

    /// The task results' contents, in call order.
    fn task_results(transcript: &Transcript) -> Vec<(bool, String)> {
        transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) if r.name == "task" => Some((r.is_error, r.content.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn subagents_in_one_batch_are_told_of_each_other_and_overlapping_scopes_are_refused() {
        let batch = tasks(&[
            scoped("c1", "general", &["src/parser"]),
            scoped("c2", "general", &["docs"]),
            scoped("c3", "general", &["src/parser/lex.rs"]),
        ]);
        let h = Harness::new(vec![batch, text("one"), text("two"), text("done")]).await;
        let (ended, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let results = task_results(&transcript);
        assert!(!results[0].0 && !results[1].0, "{results:?}");
        assert!(
            results[2].0 && results[2].1.contains("Not started"),
            "{results:?}"
        );
        // Each started subagent's first message names its own scope and the other's.
        let firsts: Vec<String> = h.requests()[1..3]
            .iter()
            .map(|r| r["messages"][1]["content"].as_str().unwrap().to_owned())
            .collect();
        let parser = firsts
            .iter()
            .find(|m| m.contains("job for general"))
            .unwrap();
        assert!(
            firsts.iter().all(|m| m.contains("Working alongside you")),
            "{firsts:?}"
        );
        assert!(parser.contains("Your scope"), "{parser}");
        assert!(
            firsts
                .iter()
                .any(|m| m.contains("files you may change: src/parser."))
        );
        assert!(
            firsts
                .iter()
                .any(|m| m.contains("files you may change: docs."))
        );
    }

    #[tokio::test]
    async fn a_scoped_subagent_cannot_write_outside_its_scope_until_it_claims_the_file() {
        let write = |id: &str| {
            reply(
                "",
                &[(id, "write", json!({"path": "notes.txt", "content": "x"}))],
                "tool_calls",
            )
        };
        let claim = reply(
            "",
            &[("k1", "claim", json!({"paths": ["notes.txt"]}))],
            "tool_calls",
        );
        let h = Harness::new(vec![
            tasks(&[scoped("c1", "general", &["src"])]),
            write("w1"),
            claim,
            write("w2"),
            text("wrote it"),
            text("done"),
        ])
        .await;
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        let said = |i: usize| last_message(&requests[i])["content"].to_string();
        assert!(said(2).contains("outside your scope"), "{}", said(2));
        assert!(said(3).contains("Granted"), "{}", said(3));
        assert!(!said(4).contains("outside your scope"), "{}", said(4));
        let written = crate::io::fs::read_string(&h.app.paths.project.join("notes.txt"));
        assert_eq!(written.unwrap().as_deref(), Some("x"));
    }

    #[tokio::test]
    async fn a_subagent_that_ends_with_finish_answers_with_its_summary() {
        let finish = reply(
            "",
            &[(
                "f1",
                "finish",
                json!({"summary": "Typed parser.py; behaviour unchanged."}),
            )],
            "tool_calls",
        );
        let h = Harness::new(vec![task("type it"), finish, text("done")]).await;
        let (ended, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let results = task_results(&transcript);
        assert_eq!(
            results[0],
            (false, "Typed parser.py; behaviour unchanged.".to_owned())
        );
    }
}
