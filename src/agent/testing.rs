//! The run loop's test harness: a local fake chat endpoint that replays scripted replies, and a
//! project to run in. Shared by every test that drives a run.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::response::IntoResponse;
use axum::routing::post;
use serde_json::{Value, json};

use crate::agent::{RunCtx, run};
use crate::api::Api;
use crate::api::models::Model;
use crate::app::App;
use crate::conversation::message::Part;
use crate::io::http::Http;
use crate::live::event::{AgentEvent, Ended};
use crate::live::state::Shared;
use crate::mode;
use crate::session::transcript::Transcript;
use crate::snapshot::Shadow;
use crate::tools::Registry;

type Script = Arc<Mutex<(VecDeque<String>, Vec<Value>)>>;

/// One scripted reply; `calls` are (id, tool, arguments).
pub fn reply(text: &str, calls: &[(&str, &str, Value)], finish: &str) -> String {
    reply_sized(text, calls, finish, 10)
}

/// An error crowbot answers with instead of a reply.
pub fn refusal(status: u16, kind: &str) -> String {
    let body = json!({"error": {"message": format!("refused: {kind}"), "type": kind}});
    format!("STATUS {status} {body}")
}

/// `reply`, as though the request had sent `input` tokens.
pub fn reply_sized(text: &str, calls: &[(&str, &str, Value)], finish: &str, input: u64) -> String {
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
    frames.push(json!({"choices": [], "usage": {"prompt_tokens": input, "completion_tokens": 2}}));
    let mut body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    body
}

pub fn text(t: &str) -> String {
    reply(t, &[], "stop")
}

pub struct Harness {
    _dir: tempfile::TempDir,
    pub app: App,
    script: Script,
    pub model: Model,
    pub shadow: Shadow,
}

impl Harness {
    pub async fn new(replies: Vec<String>) -> Self {
        let script: Script = Arc::new(Mutex::new((replies.into(), Vec::new())));
        let state = script.clone();
        let base = crate::testing::serve(|_| {
            Router::new().route(
                "/v1/chat/completions",
                post(move |body: String| {
                    let state = state.clone();
                    async move {
                        let mut s = state.lock().unwrap();
                        s.1.push(serde_json::from_str(&body).unwrap());
                        let next = s.0.pop_front().unwrap_or_else(|| text("(unscripted)"));
                        // `refusal` scripts an error status and its envelope.
                        match next.strip_prefix("STATUS ").and_then(|r| r.split_once(' ')) {
                            Some((status, body)) => (
                                axum::http::StatusCode::from_u16(status.parse().unwrap()).unwrap(),
                                [("content-type", "application/json")],
                                body.to_owned(),
                            )
                                .into_response(),
                            None => ([("content-type", "text/event-stream")], next).into_response(),
                        }
                    }
                }),
            )
        })
        .await;

        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_tests(dir.path());
        let http = Http::new(std::time::Duration::from_secs(5)).unwrap();
        app.api = Api::redirected(http, "k", &base);
        let model = crate::testing::model(false);
        Self {
            _dir: dir,
            app,
            script,
            model,
            shadow: Shadow::none("no git in these tests".into()),
        }
    }

    /// The project made a git work tree with snapshots on; `None` on a machine without git.
    pub async fn with_git(mut self) -> Option<Self> {
        let program = crate::io::proc::which("git")?;
        let init = crate::io::proc::exec(&crate::io::proc::Program {
            program: &program,
            args: &["init".into(), "--quiet".into()],
            cwd: &self.app.paths.project,
            env: &[],
            unset: &[],
            timeout: std::time::Duration::from_secs(30),
        })
        .await
        .ok()?;
        assert!(init.ok());
        self.shadow = Shadow::open(&self.app.paths).await;
        assert_eq!(self.shadow.off(), None);
        Some(self)
    }

    pub fn write(&self, path: &str, text: &str) {
        crate::io::fs::write_atomic(
            &self.app.paths.project.join(path),
            text.as_bytes(),
            crate::io::fs::Access::Shared,
        )
        .unwrap();
    }

    /// Runs one prompt in a new conversation; `on` sees every event and may drive `shared`
    /// like a frontend does.
    pub async fn run(
        &self,
        shared: &Shared,
        on: impl Fn(&AgentEvent, &Shared) + Send + Sync,
    ) -> (Ended, Transcript, Vec<Value>) {
        self.run_in(Transcript::create(&self.app.paths), shared, on)
            .await
    }

    /// `run`, in a conversation the test has made.
    pub async fn run_in(
        &self,
        mut transcript: Transcript,
        shared: &Shared,
        on: impl Fn(&AgentEvent, &Shared) + Send + Sync,
    ) -> (Ended, Transcript, Vec<Value>) {
        let registry = Registry::builtin(self.app.settings.shell.as_deref());
        let events = Mutex::new(Vec::new());
        let emit = |event: AgentEvent| {
            on(&event, shared);
            events
                .lock()
                .unwrap()
                .push(serde_json::to_value(&event).unwrap());
        };
        let cx = RunCtx {
            app: &self.app,
            model: &self.model,
            effort: None,
            system: "sys",
            tools: &registry,
            emit: &emit,
            shadow: &self.shadow,
        };
        let prompt = vec![Part::Text { text: "go".into() }];
        let ended = run::run(&cx, &mut transcript, shared, prompt).await;
        (ended, transcript, events.into_inner().unwrap())
    }

    pub fn requests(&self) -> Vec<Value> {
        self.script.lock().unwrap().1.clone()
    }
}

pub fn shared(mode: &str) -> Shared {
    Shared::new(mode::get(mode).unwrap())
}

pub fn last_message(request: &Value) -> Value {
    request["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone()
}
