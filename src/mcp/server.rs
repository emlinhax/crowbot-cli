//! One MCP server: started as the program config.toml names, introduced (`initialize`), its tools
//! listed, and stopped with the session.

use std::time::Duration;

use serde_json::json;

use crate::auth;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::io;
use crate::io::proc::child::{self, Process, Service};
use crate::mcp::config;
use crate::mcp::protocol::{self, DATA, Initialized, ToolPage};
use crate::mcp::rpc::{self, Rpc};
use crate::text::template::fill;

pub struct Server {
    pub name: String,
    /// Its tools, as it described them.
    pub tools: Vec<serde_json::Value>,
    rpc: Rpc,
    process: Process,
}

impl Server {
    /// Starts server `name` in the project and asks for its tools; `Err` says why it is not
    /// running, with the end of what it printed when it printed anything.
    pub async fn start(name: &str, cfg: &config::Server, paths: &Paths) -> Result<Self, String> {
        let text = &DATA.text;
        let program = io::proc::which(&cfg.command)
            .ok_or_else(|| fill(&text.not_found, &[("command", &cfg.command)]))?;
        let env: Vec<(String, String)> = cfg
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let limits = &limits::get().mcp;
        let running = child::start(&Service {
            program: &program,
            args: &cfg.args,
            cwd: &paths.project,
            env: &env,
            unset: auth::hidden_env(),
            line_bytes: limits.line_bytes.value,
            stderr_bytes: limits.stderr_bytes.value,
        })
        .map_err(|e| fill(&text.no_start, &[("error", &e.to_string())]))?;
        let rpc = Rpc::new(running.outgoing, running.incoming, text.closed.clone());
        let started = io::clock::timeout(limits.start_secs.secs(), handshake(&rpc)).await;
        let listed = started.unwrap_or_else(|_| {
            let secs = limits.start_secs.value.to_string();
            Err(fill(&text.timed_out, &[("secs", &secs)]))
        });
        match listed {
            Ok(tools) => Ok(Self {
                name: name.to_owned(),
                tools,
                rpc,
                process: running.process,
            }),
            Err(why) => {
                let said = said(&why, &running.process);
                running.process.stop(limits.stop_ms.ms()).await;
                Err(said)
            }
        }
    }

    /// Closes its stdin, gives it `mcp.stop_ms` to finish, then ends it.
    pub async fn stop(self) {
        drop(self.rpc);
        self.process.stop(limits::get().mcp.stop_ms.ms()).await;
    }
}

/// `why`, with the end of what the server printed on stderr, if it printed anything.
fn said(why: &str, process: &Process) -> String {
    let stderr = process.stderr_tail();
    match stderr.is_empty() {
        true => why.to_owned(),
        false => fill(&DATA.text.said, &[("why", why), ("stderr", &stderr)]),
    }
}

/// Introduces crowbot, checks the version the server answers with, and lists its tools.
pub async fn handshake(rpc: &Rpc) -> Result<Vec<serde_json::Value>, String> {
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
    Ok(tools)
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
pub fn hello(version: &str) -> serde_json::Value {
    json!({"protocolVersion": version, "capabilities": {"tools": {}}})
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tokio::sync::mpsc;

    use super::*;

    /// Plays a server over `rpc`'s channels: each request's method answered by `answer`.
    fn serve(answer: impl Fn(&str, &Value) -> Option<Value> + Send + 'static) -> Rpc {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                let message: Value = serde_json::from_str(&line).unwrap();
                let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str())
                else {
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
        let tools = handshake(&rpc).await.unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["a", "b"]);
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
        let tools = handshake(&rpc).await.unwrap();
        assert_eq!(tools.len(), limits::get().mcp.list_pages.value);
    }
}
