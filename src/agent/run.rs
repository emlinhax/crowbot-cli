//! The agent loop (after pi's agent-loop): ask the model, run what it calls, feed the results
//! back, until it stops calling tools. Messages queued meanwhile are delivered at safe points.

use crate::agent::batch::{self, Batch};
use crate::agent::context;
use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::{Finish, Message, Part, ToolCall};
use crate::agent::model_text::{self, fill};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::chat::{self, Event, Turn};
use crate::api::models::Model;
use crate::app::App;
use crate::limits;
use crate::session::transcript::Transcript;
use crate::tools::Registry;

pub struct RunCtx<'a> {
    pub app: &'a App,
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a Registry,
    /// Where PLAN mode writes its plan; the only file it may change.
    pub plan_file: String,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

pub async fn run(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    prompt: Vec<Part>,
) -> anyhow::Result<Outcome> {
    let cancel = shared.begin_run();
    transcript.push(Message::User {
        parts: with_reminder(cx, shared, prompt),
    })?;
    let tools = cx.tools.wire();
    let max_turns = limits::get().agent.max_turns.value;
    let mut turns = 0;

    let outcome = 'run: loop {
        loop {
            if turns >= max_turns {
                (cx.emit)(AgentEvent::Notice {
                    text: fill(
                        &model_text::get().max_turns,
                        &[("count", &max_turns.to_string())],
                    ),
                });
                break 'run Outcome::TurnLimit;
            }
            turns += 1;

            let messages = context::build(&transcript.messages);
            let turn = Turn {
                model: cx.model,
                effort: cx.effort,
                system: cx.system,
                messages: &messages,
                tools: &tools,
            };
            let reply = chat::stream(&cx.app.api, &turn, &cancel, &mut |event| {
                (cx.emit)(match event {
                    Event::Delta(delta) => AgentEvent::from(delta),
                    Event::Retry {
                        attempt,
                        delay,
                        error,
                    } => AgentEvent::Retry {
                        attempt,
                        delay_ms: delay.as_millis() as u64,
                        error: error.clone(),
                    },
                });
            })
            .await;
            transcript.push(Message::Assistant(reply.clone()))?;
            let finish = reply.finish;
            let calls: Vec<ToolCall> = reply.tool_calls().cloned().collect();
            (cx.emit)(AgentEvent::MessageEnd { message: reply });
            match finish {
                Finish::Aborted => break 'run Outcome::Aborted,
                Finish::Error => break 'run Outcome::Failed,
                _ => {}
            }

            if !calls.is_empty() {
                let batch = if finish == Finish::Length {
                    // A reply cut off mid-call may carry truncated arguments; never run those.
                    batch::fail_all(&calls, &model_text::get().truncated_tool_call)
                } else {
                    let history = &transcript.messages[..transcript.messages.len() - 1];
                    batch::run(cx, history, &calls, shared, &cancel).await
                };
                let Batch { results, stop } = batch;
                for result in results {
                    transcript.push(Message::Tool(result))?;
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
                    deliver(cx, shared, transcript, parts)?;
                }
                continue;
            }
            if calls.is_empty() {
                break;
            }
        }
        let queued = shared.drain_follow();
        if queued.is_empty() {
            break Outcome::Done;
        }
        for parts in queued {
            deliver(cx, shared, transcript, parts)?;
        }
    };
    (cx.emit)(AgentEvent::RunEnd { outcome });
    Ok(outcome)
}

/// Hands a message typed mid-run to the model, and tells the frontend it landed.
fn deliver(
    cx: &RunCtx<'_>,
    shared: &Shared,
    transcript: &mut Transcript,
    parts: Vec<Part>,
) -> anyhow::Result<()> {
    let text = parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (cx.emit)(AgentEvent::Delivered { text });
    transcript.push(Message::User {
        parts: with_reminder(cx, shared, parts),
    })
}

/// Mode changes reach the model as a reminder on the next user message, so the system prompt
/// (and the vendor's cache of it) never changes mid-session.
fn with_reminder(cx: &RunCtx<'_>, shared: &Shared, mut parts: Vec<Part>) -> Vec<Part> {
    if let Some(mode) = shared.take_reminder()
        && let Some(text) = mode.reminder.as_deref().and_then(system_prompt::reminder)
    {
        parts.insert(
            0,
            Part::Reminder {
                text: fill(text, &[("plan_file", &cx.plan_file)]),
            },
        );
    }
    parts
}

#[cfg(test)]
mod tests {
    //! The loop against a local fake chat endpoint that replays scripted replies.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::routing::post;
    use serde_json::{Value, json};

    use super::*;
    use crate::agent::prompt::Reply;
    use crate::api::Api;
    use crate::api::models::{Capabilities, Pricing};
    use crate::io::http::Http;
    use crate::mode;

    type Script = Arc<Mutex<(VecDeque<String>, Vec<Value>)>>;

    /// One scripted reply; `calls` are (id, tool, arguments).
    fn reply(text: &str, calls: &[(&str, &str, Value)], finish: &str) -> String {
        let mut frames = Vec::new();
        if !text.is_empty() {
            frames.push(json!({"choices": [{"index": 0, "delta": {"content": text}}]}));
        }
        for (i, (id, name, args)) in calls.iter().enumerate() {
            frames.push(json!({"choices": [{"index": 0, "delta": {"tool_calls": [
                {"index": i, "id": id, "type": "function",
                 "function": {"name": name, "arguments": args.to_string()}}]}}]}));
        }
        frames.push(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}));
        frames.push(json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 2}}));
        let mut body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn text(t: &str) -> String {
        reply(t, &[], "stop")
    }

    struct Harness {
        _dir: tempfile::TempDir,
        app: App,
        script: Script,
        model: Model,
    }

    impl Harness {
        async fn new(replies: Vec<String>) -> Self {
            let script: Script = Arc::new(Mutex::new((replies.into(), Vec::new())));
            let state = script.clone();
            let router = Router::new().route(
                "/v1/chat/completions",
                post(move |body: String| {
                    let state = state.clone();
                    async move {
                        let mut s = state.lock().unwrap();
                        s.1.push(serde_json::from_str(&body).unwrap());
                        let next = s.0.pop_front().unwrap_or_else(|| text("(unscripted)"));
                        ([("content-type", "text/event-stream")], next)
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

            let dir = tempfile::tempdir().unwrap();
            let mut app = App::for_tests(dir.path());
            let http = Http::new(std::time::Duration::from_secs(5)).unwrap();
            app.api = Api::redirected(http, "k", &base);
            let model = Model {
                id: "m".into(),
                display_name: String::new(),
                description: String::new(),
                context_window: 100_000,
                max_output_tokens: 1000,
                pricing: Pricing {
                    input_per_1m_usd: 1.0,
                    cached_input_per_1m_usd: 0.1,
                    output_per_1m_usd: 2.0,
                },
                capabilities: Capabilities::default(),
            };
            Self {
                _dir: dir,
                app,
                script,
                model,
            }
        }

        fn write(&self, path: &str, text: &str) {
            crate::io::fs::write_atomic(
                &self.app.paths.project.join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }

        /// Runs one prompt; `on` sees every event and may drive `shared` like a frontend does.
        async fn run(
            &self,
            shared: &Shared,
            on: impl Fn(&AgentEvent, &Shared) + Send + Sync,
        ) -> (Outcome, Transcript, Vec<String>) {
            let registry = Registry::builtin(&self.app);
            let events = Mutex::new(Vec::new());
            let emit = |event: AgentEvent| {
                on(&event, shared);
                let kind = serde_json::to_value(&event).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                events.lock().unwrap().push(kind);
            };
            let cx = RunCtx {
                app: &self.app,
                model: &self.model,
                effort: None,
                system: "sys",
                tools: &registry,
                plan_file: self.plan_file(),
                emit: &emit,
            };
            let mut transcript = Transcript::new(None);
            let prompt = vec![Part::Text { text: "go".into() }];
            let outcome = run(&cx, &mut transcript, shared, prompt).await.unwrap();
            (outcome, transcript, events.into_inner().unwrap())
        }

        fn plan_file(&self) -> String {
            self.app
                .paths
                .home
                .join("plan.md")
                .to_string_lossy()
                .replace('\\', "/")
        }

        fn requests(&self) -> Vec<Value> {
            self.script.lock().unwrap().1.clone()
        }
    }

    fn shared(mode: &str) -> Shared {
        Shared::new(mode::get(mode).unwrap())
    }

    fn last_message(request: &Value) -> Value {
        request["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone()
    }

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
        let (outcome, transcript, _) = h.run(&shared("manual"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
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
    async fn follow_ups_wait_until_the_model_is_done() {
        let h = Harness::new(vec![text("first"), text("second")]).await;
        let state = shared("manual");
        state.follow_up(vec![Part::Text {
            text: "and then?".into(),
        }]);
        let (outcome, _, _) = h.run(&state, |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(last_message(&requests[1])["content"], "and then?");
    }

    #[tokio::test]
    async fn a_plain_rejection_stops_the_run() {
        let h = Harness::new(vec![reply(
            "",
            &[("c1", "bash", json!({"command": "echo hi"}))],
            "tool_calls",
        )])
        .await;
        let (outcome, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.answer(*id, Reply::No { feedback: None });
                }
            })
            .await;
        assert_eq!(outcome, Outcome::Rejected);
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
                shared.answer(
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
                    json!({"question": "Which DB?", "options": ["sqlite", "postgres"]}),
                )],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        let (_, _, events) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.answer(*id, Reply::Choice(1));
                }
            })
            .await;
        assert_eq!(events.iter().filter(|e| *e == "prompt").count(), 1);
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("postgres"));
    }

    #[tokio::test]
    async fn an_approved_plan_switches_mode() {
        let h = Harness::new(vec![
            reply("", &[("c1", "plan_exit", json!({}))], "tool_calls"),
            text("implementing"),
        ])
        .await;
        crate::io::fs::write_atomic(
            std::path::Path::new(&h.plan_file()),
            b"1. do it",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let state = shared("plan");
        h.run(&state, |event, shared| {
            if let AgentEvent::Prompt { id, .. } = event {
                shared.answer(*id, Reply::Choice(0));
            }
        })
        .await;
        assert_eq!(state.mode().id, "manual");
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("MANUAL"));
    }

    #[tokio::test]
    async fn switching_to_auto_approves_a_waiting_prompt() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "bash", json!({"command": "echo hi"}))],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        let (outcome, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if matches!(event, AgentEvent::Prompt { .. }) {
                    shared.set_mode(mode::get("auto").unwrap());
                }
            })
            .await;
        assert_eq!(outcome, Outcome::Done);
        let ran = transcript
            .messages
            .iter()
            .any(|m| matches!(m, Message::Tool(r) if r.content.contains("hi")));
        assert!(ran);
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
        let (outcome, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(
            crate::io::fs::kind(&h.app.paths.project.join("new.txt")),
            crate::io::fs::Kind::Missing
        );
        let result = last_message(&h.requests()[1]);
        assert_eq!(result["role"], "tool");
        assert!(result["content"].as_str().unwrap().contains("output limit"));
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
        let (outcome, _, _) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let first = last_message(&h.requests()[0]);
        assert!(first["content"].as_str().unwrap().contains("PLAN mode"));
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("Not allowed"));
    }
}
