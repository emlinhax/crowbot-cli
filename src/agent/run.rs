//! The agent loop (after pi's agent-loop): ask the model, run what it calls, feed the results
//! back, until it stops calling tools. Messages queued meanwhile are delivered at safe points.

use crate::agent::batch::{self, Batch};
use crate::agent::compact::{self, Why};
use crate::agent::context;
use crate::agent::model_text;
use crate::agent::orient;
use crate::agent::overflow;
use crate::agent::verify;
use crate::agent::written;
use crate::agent::{Run, RunCtx};
use crate::api::chat::{self, Turn};
use crate::api::models::Model;
use crate::config::limits;
use crate::conversation::message::{Finish, Message, Part, ToolCall};
use crate::live::event::{AgentEvent, Ended, Outcome};
use crate::live::state::{Reminder, Shared};
use crate::session::entry::{Snap, State};
use crate::session::transcript::{Transcript, Unsaved};
use crate::text::template::fill;
use crate::tools;

pub async fn run(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    prompt: Vec<Part>,
) -> Ended {
    run_under(cx, transcript, shared, prompt, None).await
}

/// `run`, started by `parent`'s `task` call when it is a subagent's.
pub(crate) async fn run_under(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    prompt: Vec<Part>,
    parent: Option<&Run<'_>>,
) -> Ended {
    let run = Run {
        cx,
        shared,
        cancel: shared.begin_run(),
        plan_file: match parent {
            Some(parent) => parent.plan_file.clone(),
            None => tools::plan_file(&cx.app.paths, transcript.id()),
        },
        before: tokio::sync::OnceCell::new(),
        session: transcript.file().to_path_buf(),
        parent,
        slots: tokio::sync::Semaphore::new(limits::get().subagent.parallel.value),
    };
    let cancel = &run.cancel;
    // Hand the model the working directory and repo state on the first turn, so it starts from
    // facts instead of spending turns discovering them (or guessing). Once per conversation,
    // top-level runs only; rides on the first prompt, never the cached system prompt.
    let prompt = match parent.is_none() && shared.orient_once() {
        true => {
            let text = orient::snapshot(&cx.app.paths).await;
            let mut parts = prompt;
            if !text.is_empty() {
                parts.insert(0, Part::Reminder { text });
            }
            parts
        }
        false => prompt,
    };
    say(&run, transcript, prompt);
    let prompt_id = transcript.last_id().unwrap_or_default().to_owned();
    let mut snapped = false;
    let tools = cx.tools.wire();
    let max_turns = match parent {
        Some(_) => limits::get().subagent.max_turns.value,
        None => limits::get().agent.max_turns.value,
    };
    let mut turns = 0;
    // How many times the configured check ran and failed this run; capped so a check the model
    // cannot satisfy ends the run rather than looping it.
    let mut verified = 0;
    // One summary-and-send-again per run, and none after one failed: a model that cannot
    // summarise would only fail again.
    let (mut sent_again, mut summary_failed) = (false, false);
    // Calls written as text are pointed out once per run: a model that keeps writing them would
    // only spend turns.
    let mut told_written = false;

    let outcome = 'run: loop {
        loop {
            if turns >= max_turns {
                break 'run Outcome::TurnLimit;
            }
            turns += 1;

            // A preset may pick another model for this request; its window decides a summary.
            let now = cx.picked(transcript, shared);
            if let Some(why) = window_full(transcript, now.model).filter(|_| !summary_failed) {
                let summarised =
                    compact::compact(&now, transcript, shared, cancel, None, why).await;
                summary_failed = !matches!(summarised, Ok(true));
            }
            let (summary, tail) = transcript.context();
            let messages = context::build(summary, tail);
            let turn = Turn {
                model: now.model,
                effort: now.effort,
                system: cx.system,
                messages: &messages,
                tools: &tools,
            };
            let reply = chat::stream(&cx.app.api, &turn, cancel, &mut |event| {
                (cx.emit)(AgentEvent::from(event));
            })
            .await;
            record(cx, transcript, Message::Assistant(reply.clone()));
            let finish = reply.finish;
            let calls: Vec<ToolCall> = reply.tool_calls().cloned().collect();
            let written = calls.is_empty() && written::wrote_calls(&Part::text_of(&reply.parts));
            let refused = reply.error.as_ref().is_some_and(overflow::refused);
            (cx.emit)(AgentEvent::MessageEnd { message: reply });
            if refused && !sent_again && !summary_failed {
                sent_again = true;
                let why = Why {
                    auto: true,
                    overflow: true,
                };
                let summarised =
                    compact::compact(&now, transcript, shared, cancel, None, why).await;
                if matches!(summarised, Ok(true)) {
                    continue;
                }
                summary_failed = true;
            }
            match finish {
                Finish::Aborted => break 'run Outcome::Aborted,
                Finish::Error => break 'run Outcome::Failed,
                _ => {}
            }

            // The model ends the run by calling `finish`, if the rest of its batch succeeded.
            let mut finishing = false;
            if !calls.is_empty() {
                let batch = if finish == Finish::Length {
                    // A reply cut off mid-call may carry truncated arguments; never run those.
                    let why = &model_text::get().truncated_tool_call;
                    batch::refuse_all(&run, &calls, "truncated_tool_call", why).await
                } else {
                    let history = &transcript.messages[..transcript.messages.len() - 1];
                    batch::run(&run, history, &calls).await
                };
                let Batch { results, stop } = batch;
                finishing = calls.iter().any(|c| c.name == tools::FINISH)
                    && results
                        .iter()
                        .filter(|r| r.name != tools::FINISH)
                        .all(|r| !r.is_error);
                // The snapshot goes in the file before the changes it lets /undo take back; only
                // the frontend's run writes one, its subagents' changes included.
                if let Some(before) = run.before.get().filter(|_| !snapped && parent.is_none()) {
                    snapped = true;
                    let trees = Snap {
                        before: before.clone(),
                        after: None,
                    };
                    reported(cx, transcript.snapshot(&prompt_id, trees));
                }
                for result in results {
                    record(cx, transcript, Message::Tool(result));
                }
                if cancel.is_cancelled() {
                    break 'run Outcome::Aborted;
                }
                if stop {
                    break 'run Outcome::Rejected;
                }
            }

            let steered = shared.drain_steer();
            if !steered.is_empty() {
                for parts in steered {
                    deliver(&run, transcript, parts);
                }
                continue;
            }
            if written && !told_written && !tools.is_empty() {
                told_written = true;
                let text = model_text::get().written_calls.clone();
                let parts = vec![Part::Reminder { text }];
                record(cx, transcript, Message::User { parts });
                continue;
            }
            // `finish` ends the run through the same path as an empty reply, so auto-verify composes.
            if finishing || calls.is_empty() {
                break;
            }
        }
        let queued = shared.drain_follow();
        if queued.is_empty() {
            // Before ending: if a check is configured and this run changed files, run it; a failure
            // is handed back so the model fixes it rather than ending broken (capped, so no loop).
            let changed = run.before.get().is_some();
            if let Some(parts) = verify::failure(
                &cx.app.settings,
                &cx.app.paths,
                cancel,
                changed,
                parent.is_none(),
                verified,
            )
            .await
            {
                verified += 1;
                deliver(&run, transcript, parts);
                continue;
            }
            break Outcome::Done;
        }
        for parts in queued {
            deliver(&run, transcript, parts);
        }
    };
    if let Some(before) = run.before.get().filter(|_| parent.is_none()) {
        let trees = Snap {
            before: before.clone(),
            after: cx.shadow.take().await,
        };
        reported(cx, transcript.snapshot(&prompt_id, trees));
    }
    let ended = Ended { outcome, turns };
    (cx.emit)(AgentEvent::RunEnd(ended));
    ended
}

/// Why the conversation should be summarised before the next request, if it should: the last
/// request since the latest summary nearly filled the window, or its reply was cut off because
/// the window ran out.
fn window_full(transcript: &Transcript, model: &Model) -> Option<Why> {
    let (reply, usage) = transcript.last_used()?;
    let filled = overflow::window_filled(reply, model.context_window);
    (filled || overflow::nearly_full(usage.input, model)).then_some(Why {
        auto: true,
        overflow: filled,
    })
}

/// Keeps `message`; a session file that can no longer be written is reported once, and the
/// run goes on without it.
fn record(cx: &RunCtx<'_>, transcript: &mut Transcript, message: Message) {
    reported(cx, transcript.push(message));
}

fn reported(cx: &RunCtx<'_>, kept: Result<(), Unsaved>) {
    if let Err(unsaved) = kept {
        (cx.emit)(AgentEvent::Unsaved {
            path: unsaved.path.display().to_string(),
            error: unsaved.error,
        });
    }
}

/// Hands a message typed mid-run to the model, and tells the frontend it landed.
fn deliver(run: &Run<'_>, transcript: &mut Transcript, parts: Vec<Part>) {
    let text = Part::text_of(&parts);
    (run.cx.emit)(AgentEvent::Delivered { text });
    say(run, transcript, parts);
}

/// Records a user message, with any reminder that is due in front of it and what it is sent
/// under.
fn say(run: &Run<'_>, transcript: &mut Transcript, parts: Vec<Part>) {
    let parts = with_reminder(run, parts);
    let state = State {
        model: run.cx.chosen().to_owned(),
        effort: run.cx.effort.map(str::to_owned),
        mode: Some(run.shared.mode().id.clone()),
    };
    reported(
        run.cx,
        transcript.push_prompt(Message::User { parts }, state),
    );
}

/// Mode changes reach the model as a reminder on the next user message, so the system prompt
/// (and the vendor's cache of it) never changes mid-session.
fn with_reminder(run: &Run<'_>, mut parts: Vec<Part>) -> Vec<Part> {
    let text = match run.shared.take_reminder() {
        Some(Reminder::Entered(mode)) => mode
            .reminder
            .as_deref()
            .map(|text| fill(text.trim_end(), &[("plan_file", &run.plan_file)])),
        Some(Reminder::Left { now }) => {
            Some(fill(&model_text::get().mode_left, &[("mode", &now.label)]))
        }
        None => None,
    };
    if let Some(text) = text {
        parts.insert(0, Part::Reminder { text });
    }
    parts
}

#[cfg(test)]
mod tests {
    //! The loop against a local fake chat endpoint that replays scripted replies.

    use serde_json::{Value, json};

    use super::*;
    use crate::agent::testing::{Harness, last_message, refusal, reply, reply_sized, shared, text};
    use std::sync::Mutex;

    use crate::conversation::message::Assistant;
    use crate::live::prompt::{Answer, Prompt, Reply};
    use crate::mode;

    #[tokio::test]
    async fn parallel_results_come_back_in_call_order() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "read", json!({"path": "a.txt"})),
                    ("c2", "read", json!({"path": "b.txt"})),
                ],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        h.write("a.txt", "AAA");
        h.write("b.txt", "BBB");
        let (ended, transcript, _) = h.run(&shared("manual"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        // One turn made the calls and one read their results.
        assert_eq!(ended.turns, 2);
        let results: Vec<&str> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some(r.call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(results, vec!["c1", "c2"]);
        let roles: Vec<String> = h.requests()[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "tool", "tool"]);
    }

    #[tokio::test]
    async fn steering_lands_after_the_tools_and_before_the_next_turn() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "read", json!({"path": "a.txt"}))],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        h.write("a.txt", "x");
        h.run(&shared("manual"), |event, shared| {
            if matches!(event, AgentEvent::ToolStart { .. }) {
                shared.steer(vec![Part::Text {
                    text: "also look at b".into(),
                }]);
            }
        })
        .await;
        let last = last_message(&h.requests()[1]);
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"], "also look at b");
    }

    #[tokio::test]
    async fn calls_written_as_text_are_pointed_out_once() {
        let written =
            r#"I'll read it. {"tool_calls":[{"name":"read","arguments":{"path":"a.txt"}}]}"#;
        let h = Harness::new(vec![
            text(written),
            reply(
                "",
                &[("c1", "read", json!({"path": "a.txt"}))],
                "tool_calls",
            ),
            text(written),
        ])
        .await;
        h.write("a.txt", "x");
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        // Told once, then the call is made; written out again, the run ends there.
        assert_eq!(requests.len(), 3);
        let told = last_message(&requests[1]);
        assert_eq!(told["role"], "user");
        let told = told["content"].to_string();
        assert!(told.contains("wrote tool calls into its text"), "{told}");
    }

    #[tokio::test]
    async fn follow_ups_wait_until_the_model_is_done() {
        let h = Harness::new(vec![text("first"), text("second")]).await;
        let state = shared("manual");
        state.follow_up(vec![Part::Text {
            text: "and then?".into(),
        }]);
        let (ended, _, _) = h.run(&state, |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(last_message(&requests[1])["content"], "and then?");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_check_continues_the_run_until_the_model_fixes_it() {
        // The model changes a file and stops; the configured check fails until an `ok.flag` exists,
        // so its failure is handed back; the model creates the flag, the check passes, the run ends.
        // Needs git: the changed-files gate rides on the run's snapshot (shadow), which is off
        // without a work tree.
        let scripted = vec![
            reply(
                "",
                &[("c1", "write", json!({"path": "work.txt", "content": "x"}))],
                "tool_calls",
            ),
            text("I think I'm done."),
            reply(
                "",
                &[("c2", "write", json!({"path": "ok.flag", "content": "x"}))],
                "tool_calls",
            ),
            text("Fixed and done."),
        ];
        let Some(mut h) = Harness::new(scripted).await.with_git().await else {
            return; // no git on this machine
        };
        h.app.settings.verify = Some("test -f ok.flag".into());
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let sent = h
            .requests()
            .iter()
            .map(ToString::to_string)
            .collect::<String>();
        assert!(
            sent.contains("test -f ok.flag") && sent.contains("failed after your changes"),
            "the failing check should have been handed back: {sent}"
        );
    }

    #[tokio::test]
    async fn finish_ends_a_successful_batch_with_no_further_turn() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "write", json!({"path": "ok.txt", "content": "x"})),
                    ("c2", "finish", json!({"summary": "wrote it"})),
                ],
                "tool_calls",
            ),
            text("this reply must not run"),
        ])
        .await;
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        assert_eq!(ended.turns, 1, "the run ended after the finishing batch");
        assert_eq!(h.requests().len(), 1, "no further turn was taken");
    }

    #[tokio::test]
    async fn finish_does_not_end_the_run_when_a_sibling_call_fails() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "read", json!({"path": "nope.txt"})), // missing file: errors
                    ("c2", "finish", json!({})),
                ],
                "tool_calls",
            ),
            text("recovered"),
        ])
        .await;
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        assert_eq!(
            h.requests().len(),
            2,
            "a failed sibling means finish is ignored and the run goes on"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_finish_still_runs_a_configured_verify() {
        // finish after a change, but a configured check fails until `ok.flag` exists: the run must
        // continue (finish routes through auto-verify, not around it).
        let scripted = vec![
            reply(
                "",
                &[
                    ("c1", "write", json!({"path": "work.txt", "content": "x"})),
                    ("c2", "finish", json!({})),
                ],
                "tool_calls",
            ),
            reply(
                "",
                &[("c3", "write", json!({"path": "ok.flag", "content": "x"}))],
                "tool_calls",
            ),
            text("done now"),
        ];
        let Some(mut h) = Harness::new(scripted).await.with_git().await else {
            return; // no git on this machine
        };
        h.app.settings.verify = Some("test -f ok.flag".into());
        let (ended, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let sent = h
            .requests()
            .iter()
            .map(ToString::to_string)
            .collect::<String>();
        assert!(
            sent.contains("failed after your changes"),
            "the configured verify ran after finish: {sent}"
        );
    }

    #[tokio::test]
    async fn a_plain_rejection_stops_the_run() {
        let h = Harness::new(vec![reply(
            "",
            &[("c1", "bash", json!({"command": "printf hi"}))],
            "tool_calls",
        )])
        .await;
        let (ended, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.prompts.answer(*id, Reply::No { feedback: None });
                }
            })
            .await;
        assert_eq!(ended.outcome, Outcome::Rejected);
        assert_eq!(h.requests().len(), 1);
        assert!(matches!(transcript.messages.last(), Some(Message::Tool(r)) if r.is_error));
    }

    #[tokio::test]
    async fn feedback_on_a_rejection_reaches_the_model() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "bash", json!({"command": "rm x"}))],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.run(&shared("manual"), |event, shared| {
            if let AgentEvent::Prompt { id, .. } = event {
                shared.prompts.answer(
                    *id,
                    Reply::No {
                        feedback: Some("use trash instead".into()),
                    },
                );
            }
        })
        .await;
        let last = last_message(&h.requests()[1]);
        assert!(
            last["content"]
                .as_str()
                .unwrap()
                .contains("use trash instead")
        );
    }

    #[tokio::test]
    async fn the_question_tool_carries_the_answer_back() {
        let h = Harness::new(vec![
            reply(
                "",
                &[(
                    "c1",
                    "question",
                    json!({"questions": [
                        {"question": "Which DB?", "options": ["sqlite", "postgres"]},
                        {"question": "Which checks?", "options": ["lint", "test"], "multiple": true}
                    ]}),
                )],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        let (_, _, events) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    let answers = vec![
                        Answer {
                            picked: vec![1],
                            text: None,
                        },
                        Answer {
                            picked: vec![0, 1],
                            text: None,
                        },
                    ];
                    shared.prompts.answer(*id, Reply::Answers(answers));
                }
            })
            .await;
        assert_eq!(events.iter().filter(|e| e["type"] == "prompt").count(), 1);
        let result = last_message(&h.requests()[1]);
        let content = result["content"].as_str().unwrap();
        assert!(content.contains("Which DB?\n  postgres"), "{content}");
        assert!(content.contains("Which checks?\n  lint, test"), "{content}");
    }

    #[tokio::test]
    async fn an_approved_plan_switches_mode() {
        let h = Harness::new(vec![
            reply("", &[("c1", "plan_exit", json!({}))], "tool_calls"),
            text("implementing"),
        ])
        .await;
        let transcript = Transcript::create(&h.app.paths);
        crate::io::fs::write_atomic(
            &h.app.paths.plan_file(transcript.id()),
            b"1. do it",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let state = shared("plan");
        h.run_in(transcript, &state, |event, shared| {
            if let AgentEvent::Prompt { id, .. } = event {
                shared.prompts.answer(*id, Reply::Choice(0));
            }
        })
        .await;
        assert_eq!(state.mode().id, "manual");
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("MANUAL"));
    }

    #[tokio::test]
    async fn switching_to_auto_approves_a_waiting_prompt_and_the_calls_after_it() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "bash", json!({"command": "printf hi"})),
                    ("c2", "bash", json!({"command": "printf there"})),
                ],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        let (ended, transcript, events) = h
            .run(&shared("manual"), |event, shared| {
                if matches!(event, AgentEvent::Prompt { .. }) {
                    shared.set_mode(mode::get("auto").unwrap());
                }
            })
            .await;
        assert_eq!(ended.outcome, Outcome::Done);
        assert_eq!(events.iter().filter(|e| e["type"] == "prompt").count(), 1);
        for said in ["hi", "there"] {
            let ran = transcript
                .messages
                .iter()
                .any(|m| matches!(m, Message::Tool(r) if r.content.contains(said)));
            assert!(ran, "{said}");
        }
    }

    /// A forum post, to a stored forum on a port nothing listens on, so an approved post fails at
    /// once without leaving the machine.
    async fn posting() -> Harness {
        let h = Harness::new(vec![
            reply(
                "",
                &[(
                    "c1",
                    "forum_post",
                    json!({"forum": "Closed", "section": "1", "topic": "2", "body": "hi"}),
                )],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        let forum = crate::forums::Forum {
            id: "7".into(),
            name: "Closed".into(),
            base_url: "http://127.0.0.1:9/forum".into(),
            mobiquo_dir: "mobiquo".into(),
            ext: "php".into(),
            kind: "mobiquo".into(),
            ..Default::default()
        };
        crate::forums::store::add(&h.app.paths, forum).unwrap();
        h
    }

    #[tokio::test]
    async fn a_forum_post_asks_once_even_in_auto() {
        let h = posting().await;
        let confirmed = Mutex::new(Vec::new());
        let (_, _, events) = h
            .run(&shared("auto"), |event, shared| {
                if let AgentEvent::Prompt { id, prompt, .. } = event {
                    if let Prompt::Permission { confirm, .. } = prompt {
                        confirmed.lock().unwrap().push(*confirm);
                    }
                    shared.prompts.answer(*id, Reply::Yes);
                }
            })
            .await;
        assert_eq!(events.iter().filter(|e| e["type"] == "prompt").count(), 1);
        // The prompt says so, so a frontend words it as a confirmation.
        assert_eq!(*confirmed.lock().unwrap(), [true]);
    }

    #[tokio::test]
    async fn a_headless_run_never_posts() {
        let h = posting().await;
        let (_, transcript, _) = h
            .run(&shared("auto"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.prompts.answer(*id, Reply::Unavailable);
                }
            })
            .await;
        let refused = transcript.messages.iter().any(
            |m| matches!(m, Message::Tool(r) if r.is_error && r.content.contains("confirm it")),
        );
        assert!(refused);
    }

    #[tokio::test]
    async fn calls_in_a_cut_off_reply_are_never_run() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "write", json!({"path": "new.txt", "content": "x"}))],
                "length",
            ),
            text("ok"),
        ])
        .await;
        let (ended, _, events) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        for kind in ["tool_start", "tool_end"] {
            assert_eq!(
                events.iter().filter(|e| e["type"] == *kind).count(),
                1,
                "{kind}"
            );
        }
        assert_eq!(
            crate::io::fs::kind(&h.app.paths.project.join("new.txt")).unwrap(),
            crate::io::fs::Kind::Missing
        );
        let result = last_message(&h.requests()[1]);
        assert_eq!(result["role"], "tool");
        assert!(result["content"].as_str().unwrap().contains("output limit"));
    }

    #[tokio::test]
    async fn results_record_what_was_asked_and_why_a_call_was_refused() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "read", json!({"path": "a.txt"})),
                    ("c2", "nope", json!({})),
                    ("c3", "bash", json!({"command": "printf hi"})),
                ],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.write("a.txt", "x");
        let (_, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.prompts.answer(*id, Reply::Unavailable);
                }
            })
            .await;
        let results: Vec<_> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some((r.asks.clone(), r.refused.as_deref())),
                _ => None,
            })
            .collect();
        let read = crate::permission::gate::Ask::new("read", "a.txt");
        let bash = crate::permission::gate::Ask::new("bash", "printf hi");
        assert_eq!(
            results,
            [
                (vec![read], None),
                (vec![], Some("unknown_tool")),
                (vec![bash], Some("non_interactive")),
            ]
        );
    }

    #[tokio::test]
    async fn repeated_identical_calls_are_stopped_in_auto() {
        let call = |id| reply("", &[(id, "read", json!({"path": "a.txt"}))], "tool_calls");
        let h = Harness::new(vec![call("c1"), call("c2"), call("c3"), text("done")]).await;
        h.write("a.txt", "x");
        h.run(&shared("auto"), |_, _| {}).await;
        let third = last_message(&h.requests()[3]);
        assert!(
            third["content"].as_str().unwrap().contains("3 times"),
            "{third}"
        );
    }

    #[tokio::test]
    async fn a_run_that_changes_files_is_snapshotted_once_and_a_reading_one_never() {
        let write = |path: &str| {
            reply(
                "",
                &[("c1", "write", json!({"path": path, "content": "x"}))],
                "tool_calls",
            )
        };
        let read = reply(
            "",
            &[("c1", "read", json!({"path": "a.txt"}))],
            "tool_calls",
        );
        let replies = vec![
            write("one.txt"),
            write("two.txt"),
            text("done"),
            read,
            text("read"),
        ];
        let Some(h) = Harness::new(replies).await.with_git().await else {
            return;
        };
        h.write("a.txt", "a");
        let (_, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        let path = transcript.path().unwrap().to_path_buf();
        let text = crate::io::fs::read_string(&path).unwrap().unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let snaps: Vec<&Value> = lines.iter().filter(|l| l["type"] == "snapshot").collect();
        // One before the first write, then the same one again with the run's end.
        assert_eq!(snaps.len(), 2, "{text}");
        assert_eq!(snaps[0]["before"], snaps[1]["before"]);
        assert!(snaps[0]["after"].is_null() && snaps[1]["after"].is_string());
        assert_ne!(snaps[1]["before"], snaps[1]["after"]);
        let first_snapshot = lines.iter().position(|l| l["type"] == "snapshot").unwrap();
        let first_result = lines
            .iter()
            .position(|l| l["message"]["role"] == "tool")
            .unwrap();
        assert!(first_snapshot < first_result);

        let (_, transcript, _) = h
            .run_in(
                Transcript::open(&path).unwrap().0,
                &shared("auto"),
                |_, _| {},
            )
            .await;
        let text = crate::io::fs::read_string(transcript.path().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(text.matches(r#""type":"snapshot""#).count(), 2, "{text}");
    }

    /// A conversation long enough that a summary has something to stand for.
    fn long(h: &Harness) -> Transcript {
        let mut transcript = Transcript::create(&h.app.paths);
        let big = "x".repeat(limits::get().compaction.keep_tokens.value * 2);
        for i in 0..3 {
            transcript
                .push(Message::user_text(format!("question {i} {big}")))
                .unwrap();
            let text = format!("answer {i} {big}");
            let reply = Assistant::stub(Finish::Done, vec![Part::Text { text }]);
            transcript.push(Message::Assistant(reply)).unwrap();
        }
        transcript
    }

    fn asked_to_summarise(request: &Value) -> bool {
        last_message(request)["content"]
            .as_str()
            .is_some_and(|c| c.contains("call no tools"))
    }

    #[tokio::test]
    async fn a_request_that_nearly_filled_the_window_is_summarised_before_the_next() {
        let call = [("c1", "read", json!({"path": "a.txt"}))];
        let h = Harness::new(vec![
            reply_sized("", &call, "tool_calls", 90_000),
            text("SUMMARY of the questions"),
            text("done"),
        ])
        .await;
        h.write("a.txt", "a");
        let (ended, transcript, events) = h.run_in(long(&h), &shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        assert!(!asked_to_summarise(&requests[0]));
        assert!(asked_to_summarise(&requests[1]));
        assert!(
            requests[2]["messages"]
                .to_string()
                .contains("SUMMARY of the questions")
        );
        assert!(!requests[2]["messages"].to_string().contains("question 0"));
        assert!(events.iter().any(|e| e["type"] == "compaction_end"));
        assert!(transcript.context().0.is_some());
    }

    #[tokio::test]
    async fn a_request_refused_as_too_long_is_summarised_and_sent_once_more() {
        let h = Harness::new(vec![
            refusal(413, "body_too_large"),
            text("SUMMARY"),
            text("done"),
        ])
        .await;
        let (ended, _, _) = h.run_in(long(&h), &shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 3);
        assert!(asked_to_summarise(&requests[1]));
        assert!(requests[2]["messages"].to_string().contains("SUMMARY"));

        // Refused again: no third try.
        let h = Harness::new(vec![
            refusal(413, "body_too_large"),
            text("SUMMARY"),
            refusal(413, "body_too_large"),
            text("never asked"),
        ])
        .await;
        let (ended, _, _) = h.run_in(long(&h), &shared("auto"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Failed);
        assert_eq!(h.requests().len(), 3);
    }

    #[tokio::test]
    async fn plan_mode_reminds_the_model_and_refuses_edits() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "write", json!({"path": "x.txt", "content": "x"}))],
                "tool_calls",
            ),
            text("planned"),
        ])
        .await;
        let (ended, _, _) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(ended.outcome, Outcome::Done);
        let first = last_message(&h.requests()[0]);
        assert!(first["content"].as_str().unwrap().contains("PLAN mode"));
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("Not allowed"));
    }

    #[tokio::test]
    async fn a_refusal_reaches_the_model_as_what_it_is() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "write", json!({"path": "d", "content": "x"})),
                    ("c2", "write", json!({"path": "e.txt"})),
                ],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.write("d/inner.txt", "x");
        h.run(&shared("auto"), |_, _| {}).await;
        let messages = h.requests()[1]["messages"].as_array().unwrap().clone();
        let results: Vec<&str> = messages[messages.len() - 2..]
            .iter()
            .map(|m| m["content"].as_str().unwrap())
            .collect();
        assert_eq!(results[0], "d is a directory.");
        assert_eq!(
            results[1]
                .to_lowercase()
                .matches("invalid arguments")
                .count(),
            1,
            "{}",
            results[1]
        );
    }

    #[tokio::test]
    async fn leaving_plan_mode_is_told_to_the_model_once() {
        let h = Harness::new(Vec::new()).await;
        let told = |i: usize| {
            let content = last_message(&h.requests()[i])["content"].to_string();
            content.contains("no longer apply")
        };
        let session = shared("plan");
        h.run(&session, |_, _| {}).await;
        session.set_mode(mode::get("manual").unwrap());
        h.run(&session, |_, _| {}).await;
        h.run(&session, |_, _| {}).await;
        assert_eq!([told(0), told(1), told(2)], [false, true, false]);

        let session = shared("manual");
        h.run(&session, |_, _| {}).await;
        session.set_mode(mode::get("auto").unwrap());
        h.run(&session, |_, _| {}).await;
        assert_eq!([told(3), told(4)], [false, false]);
    }
}
