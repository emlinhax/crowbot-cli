//! One MCP server: started as the program config.toml names or reached at its URL, introduced
//! (`initialize`), its tools listed, called, and stopped with the session.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::auth;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::io;
use crate::io::http::Http;
use crate::io::proc::child::{self, Process, Service, Stderr};
use crate::mcp::config::{self, Reach};
use crate::mcp::http;
use crate::mcp::protocol::{self, CallResult, DATA, Initialized, ToolInfo, ToolPage};
use crate::mcp::rpc::{self, Rpc};
use crate::text::template::fill;
use crate::text::wildcard::wildcard;

pub struct Server {
    pub name: String,
    /// The tools it offers: those it listed that its `tools` setting lets through.
    pub tools: Vec<ToolInfo>,
    /// What it said of how to use them, when it said anything.
    pub instructions: Option<String>,
    caller: Caller,
    link: Link,
}

/// What crowbot holds of a running server, and so what stopping it takes.
enum Link {
    /// A program crowbot started; boxed, as on Windows its job handle makes it the far larger.
    Program(Box<Process>),
    /// A server at a URL: the task carrying crowbot's messages to it.
    Url(http::Connection),
}

/// The way to call a server's tools, which they all share.
#[derive(Clone)]
pub struct Caller {
    server: String,
    rpc: Arc<Rpc>,
    /// How long a call may take.
    limit: Duration,
    /// What a program crowbot started prints on stderr; a server at a URL has none.
    stderr: Option<Stderr>,
    /// Whether a call has said yet that the server stopped.
    told_stopped: Arc<AtomicBool>,
}

/// Why a call came back with no answer.
#[derive(Debug)]
pub struct Failed {
    pub why: String,
    /// That the server stopped, for the first call to find it so: said once a session.
    // CEILING: a server that stops stays stopped. Upgrade: start it again once, here.
    pub stopped: Option<String>,
}

impl Server {
    /// Starts server `name` in the project, or reaches it at its URL, and asks for its tools;
    /// `Err` says why it is not running, with the end of what it printed when it printed anything.
    pub async fn start(
        name: &str,
        cfg: &config::Server,
        paths: &Paths,
        web: &Http,
    ) -> Result<Self, String> {
        let text = &DATA.text;
        let limits = &limits::get().mcp;
        let limit = cfg
            .timeout_secs
            .map_or(limits.call_secs.secs(), Duration::from_secs);
        let (rpc, link) = match &cfg.reach {
            Reach::Program { command, args, env } => {
                let running = run(command, args, env, paths)?;
                let rpc = Rpc::new(running.outgoing, running.incoming, text.closed.clone());
                (rpc, Link::Program(Box::new(running.process)))
            }
            Reach::Url { url, headers } => {
                let opened = http::open(web, url, headers, limit);
                let rpc = Rpc::new(opened.outgoing, opened.incoming, text.closed.clone());
                (rpc, Link::Url(opened.connection))
            }
        };
        let started = io::clock::timeout(limits.start_secs.secs(), handshake(&rpc)).await;
        let listed = started.unwrap_or_else(|_| {
            let secs = limits.start_secs.value.to_string();
            Err(fill(&text.timed_out, &[("secs", &secs)]))
        });
        let stderr = match &link {
            Link::Program(process) => Some(process.stderr()),
            Link::Url(_) => None,
        };
        match listed {
            Ok(introduced) => Ok(Self {
                name: name.to_owned(),
                tools: offered(introduced.tools, cfg.tools.as_deref()),
                instructions: introduced.instructions,
                caller: Caller {
                    server: name.to_owned(),
                    rpc: Arc::new(rpc),
                    limit,
                    stderr,
                    told_stopped: Arc::default(),
                },
                link,
            }),
            Err(why) => {
                link.stop().await;
                Err(said(&why, stderr.as_ref()).await)
            }
        }
    }

    pub fn caller(&self) -> Caller {
        self.caller.clone()
    }

    /// Whether it has stopped since it started.
    pub fn stopped(&self) -> bool {
        self.caller.rpc.closed().is_some()
    }

    /// Closes its stdin, gives it `mcp.stop_ms` to finish, then ends it; a server at a URL is
    /// only let go.
    pub async fn stop(self) {
        self.link.stop().await;
    }
}

impl Link {
    async fn stop(self) {
        match self {
            Self::Program(process) => process.stop(limits::get().mcp.stop_ms.ms()).await,
            Self::Url(connection) => drop(connection),
        }
    }
}

/// Starts `command` in the project, its environment `env` and never crowbot's key.
fn run(
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    paths: &Paths,
) -> Result<child::Running, String> {
    let text = &DATA.text;
    let program =
        io::proc::which(command).ok_or_else(|| fill(&text.not_found, &[("command", command)]))?;
    let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let limits = &limits::get().mcp;
    child::start(&Service {
        program: &program,
        args,
        cwd: &paths.project,
        env: &env,
        unset: auth::hidden_env(),
        line_bytes: limits.line_bytes.value,
        stderr_bytes: limits.stderr_bytes.value,
    })
    .map_err(|e| fill(&text.no_start, &[("error", &e.to_string())]))
}

impl Caller {
    /// Calls its tool `tool`, by the server's name for it. A server that stopped fails it at
    /// once, with the end of what it printed.
    pub async fn call(&self, tool: &str, arguments: Value) -> Result<CallResult, Failed> {
        let params = protocol::call(tool, arguments);
        let failed = |why| Failed { why, stopped: None };
        match self.rpc.request("tools/call", params, self.limit).await {
            Ok(answer) => serde_json::from_value(answer).map_err(|e| failed(e.to_string())),
            Err(rpc::Error::Closed(why)) => {
                let why = said(&why, self.stderr.as_ref()).await;
                let first = !self.told_stopped.swap(true, Ordering::Relaxed);
                let stopped = first.then(|| {
                    fill(
                        &DATA.text.stopped,
                        &[("server", &self.server), ("why", &why)],
                    )
                });
                Err(Failed { why, stopped })
            }
            Err(e) => Err(failed(why(&e, self.limit))),
        }
    }
}

/// The tools `allowed` names, by the server's names; all of them without it.
fn offered(tools: Vec<ToolInfo>, allowed: Option<&[String]>) -> Vec<ToolInfo> {
    let Some(allowed) = allowed else {
        return tools;
    };
    tools
        .into_iter()
        .filter(|t| allowed.iter().any(|pattern| wildcard(pattern, &t.name)))
        .collect()
}

/// `why`, with the end of what the server printed on stderr, if it printed anything.
async fn said(why: &str, stderr: Option<&Stderr>) -> String {
    let Some(stderr) = stderr else {
        return why.to_owned();
    };
    let stderr = stderr.tail(limits::get().mcp.stop_ms.ms()).await;
    match stderr.is_empty() {
        true => why.to_owned(),
        false => fill(&DATA.text.said, &[("why", why), ("stderr", &stderr)]),
    }
}

/// What a server told crowbot when introduced.
#[derive(Debug)]
pub struct Introduced {
    pub tools: Vec<ToolInfo>,
    pub instructions: Option<String>,
}

/// Introduces crowbot, checks the version the server answers with, and lists its tools.
pub async fn handshake(rpc: &Rpc) -> Result<Introduced, String> {
    let wait = limits::get().mcp.start_secs.secs();
    let answer = rpc
        .request("initialize", protocol::initialize(), wait)
        .await
        .map_err(|e| why(&e, wait))?;
    let hello: Initialized = serde_json::from_value(answer).map_err(|e| e.to_string())?;
    if !DATA.accepted.contains(&hello.protocol_version) {
        let accepted = DATA.accepted.join(", ");
        return Err(fill(
            &DATA.text.version,
            &[
                ("version", &hello.protocol_version),
                ("accepted", &accepted),
            ],
        ));
    }
    rpc.notify("notifications/initialized", json!({}));
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..limits::get().mcp.list_pages.value {
        let params = cursor
            .as_ref()
            .map_or(json!({}), |c| json!({ "cursor": c }));
        let page: ToolPage = rpc
            .request("tools/list", params, wait)
            .await
            .map_err(|e| why(&e, wait))
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))?;
        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(Introduced {
        tools,
        instructions: hello.instructions.filter(|text| !text.trim().is_empty()),
    })
}

/// An RPC failure in the words /status and the start notice use.
fn why(error: &rpc::Error, waited: Duration) -> String {
    match error {
        rpc::Error::Closed(why) | rpc::Error::Remote(why) => why.clone(),
        rpc::Error::TimedOut => {
            let secs = waited.as_secs().to_string();
            fill(&DATA.text.timed_out, &[("secs", &secs)])
        }
    }
}

/// A server's answer to `initialize`, for tests that play the server.
#[cfg(test)]
pub fn hello(version: &str) -> Value {
    json!({"protocolVersion": version, "capabilities": {"tools": {}}})
}

/// Plays a server over an `Rpc`'s channels: each request's method and params answered by
/// `answer`, or left unanswered when it gives `None`.
#[cfg(test)]
fn serve(answer: impl Fn(&str, &Value) -> Option<Value> + Send + 'static) -> Rpc {
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            let message: Value = serde_json::from_str(&line).unwrap();
            let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str()) else {
                continue;
            };
            if let Some(result) = answer(method, &message["params"]) {
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                let _ = in_tx.send(Ok(reply.to_string()));
            }
        }
    });
    Rpc::new(out_tx, in_rx, "it stopped".into())
}

/// A caller whose server is played by `answer`, as `serve` plays one; a call it leaves
/// unanswered times out within a second.
#[cfg(test)]
pub fn playing(answer: impl Fn(&str, &Value) -> Option<Value> + Send + 'static) -> Caller {
    played(serve(answer))
}

/// A caller whose server has stopped already.
#[cfg(test)]
pub fn stopped() -> Caller {
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    drop((out_rx, in_tx));
    played(Rpc::new(out_tx, in_rx, "it exited".into()))
}

#[cfg(test)]
fn played(rpc: Rpc) -> Caller {
    Caller {
        server: "fake".into(),
        rpc: Arc::new(rpc),
        limit: Duration::from_secs(1),
        stderr: None,
        told_stopped: Arc::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_handshake_follows_pages_of_tools() {
        let rpc = serve(|method, params| match method {
            "initialize" => Some(hello("2025-06-18")),
            "tools/list" if params.get("cursor").is_none() => {
                Some(json!({"tools": [{"name": "a"}], "nextCursor": "2"}))
            }
            "tools/list" => Some(json!({"tools": [{"name": "b"}]})),
            _ => None,
        });
        let tools = handshake(&rpc).await.unwrap().tools;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[tokio::test]
    async fn calls_to_a_server_that_stopped_fail_and_only_the_first_says_so() {
        let caller = stopped();
        let first = caller.call("echo", json!({})).await.unwrap_err();
        let second = caller.call("echo", json!({})).await.unwrap_err();
        let note = first.stopped.unwrap();
        assert!(note.starts_with("MCP server fake stopped"), "{note}");
        assert!(second.stopped.is_none(), "{second:?}");
    }

    #[test]
    fn a_tools_setting_offers_only_what_it_names() {
        let listed: Vec<ToolInfo> = ["get_issue", "get_pr", "delete_repo"]
            .iter()
            .map(|name| serde_json::from_value(json!({"name": name})).unwrap())
            .collect();
        let names = |allowed: Option<&[String]>| -> Vec<String> {
            offered(listed.clone(), allowed)
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        assert_eq!(names(None).len(), 3);
        let allowed = ["get_*".to_owned()];
        assert_eq!(names(Some(&allowed)), ["get_issue", "get_pr"]);
    }

    #[tokio::test]
    async fn a_server_speaking_another_version_is_refused() {
        let rpc = serve(|method, _| (method == "initialize").then(|| hello("1999-01-01")));
        let why = handshake(&rpc).await.unwrap_err();
        assert!(
            why.contains("1999-01-01") && why.contains("2025-06-18"),
            "{why}"
        );
    }

    #[tokio::test]
    async fn a_server_that_lists_forever_is_cut_off() {
        let rpc = serve(|method, _| match method {
            "initialize" => Some(hello("2025-06-18")),
            _ => Some(json!({"tools": [{"name": "again"}], "nextCursor": "more"})),
        });
        let tools = handshake(&rpc).await.unwrap().tools;
        assert_eq!(tools.len(), limits::get().mcp.list_pages.value);
    }
}
