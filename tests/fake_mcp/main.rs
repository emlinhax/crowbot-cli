//! A stand-in MCP server: JSON-RPC over stdin and stdout, one message a line. Without `--server`
//! it offers the fixed tools the e2e suite calls (builtin.rs); with `--server <file>` it plays the
//! server that file describes (sim/), as the MCP battery does, and `--record <dir>` keeps its state
//! and every call there. `--exit` fails at once, saying so on stderr; `--log <file>` writes down
//! each message it hears, a line each; `--instructions <text>` tells the client how to use its
//! tools.
// A test program talking on stdin and stdout; the io-wrapper rule is for crowbot itself.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

mod builtin;
mod sim;

use std::fs::File;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

/// How a `tools/call` is answered.
pub enum Call {
    Answer(Value),
    /// Left unanswered, as a tool still at work.
    Never,
    Unknown,
}

/// The tools on offer and how they answer: the fixed set, or the one a file describes.
enum Server {
    Builtin,
    Sim(Box<sim::Server>),
}

impl Server {
    /// One page of `tools/list` from `cursor`.
    fn list(&self, cursor: Option<&str>) -> Value {
        match self {
            Self::Builtin => json!({"tools": builtin::tools()}),
            Self::Sim(sim) => sim.list(cursor),
        }
    }

    fn call(&mut self, params: &Value) -> Call {
        match self {
            Self::Builtin => builtin::call(params),
            Self::Sim(sim) => sim.call(params),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--exit") {
        eprintln!("fake-mcp: told to fail at start");
        std::process::exit(1);
    }
    let mut log = flag("--log").map(|path| File::create(path).expect("the log can be written"));
    let mut server = match flag("--server") {
        Some(file) => Server::Sim(Box::new(sim::Server::load(
            &PathBuf::from(file),
            flag("--record").map(PathBuf::from),
        ))),
        None => Server::Builtin,
    };
    let instructions = flag("--instructions").or_else(|| match &server {
        Server::Sim(sim) => sim.instructions(),
        Server::Builtin => None,
    });
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(log) = &mut log {
            let _ = writeln!(log, "{}", heard(&message));
        }
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let answer = match message["method"].as_str() {
            Some("initialize") => Ok(json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake", "version": "0"},
                "instructions": instructions,
            })),
            Some("tools/list") => Ok(server.list(message["params"]["cursor"].as_str())),
            Some("tools/call") => match server.call(&message["params"]) {
                Call::Answer(result) => Ok(result),
                Call::Never => continue,
                Call::Unknown => Err(json!({"code": -32602, "message": "no such tool"})),
            },
            _ => Err(json!({"code": -32601, "message": "no such method"})),
        };
        let message = match answer {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }
}

/// A message as the log has it: its method, then the tool called or the request cancelled.
fn heard(message: &Value) -> String {
    let method = message["method"].as_str().unwrap_or("(an answer)");
    let params = &message["params"];
    match method {
        "tools/call" => format!("{method} {}", params["name"].as_str().unwrap_or_default()),
        "notifications/cancelled" => format!("{method} {}", params["requestId"]),
        _ => method.to_owned(),
    }
}
