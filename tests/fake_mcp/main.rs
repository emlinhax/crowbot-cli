//! A stand-in MCP server for crowbot's e2e suite: JSON-RPC over stdin and stdout, one message a
//! line, offering the tools in `tools()`. `--exit` fails at once, saying so on stderr; `--log
//! <file>` writes down each message it hears, a line each, for a scenario to read back;
//! `--instructions <text>` tells the client how to use its tools.
// A test program talking on stdin and stdout; the io-wrapper rule is for crowbot itself.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::fs::File;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--exit") {
        eprintln!("fake-mcp: told to fail at start");
        std::process::exit(1);
    }
    let mut log = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1))
        .map(|path| File::create(path).expect("the log can be written"));
    let instructions = args
        .iter()
        .position(|a| a == "--instructions")
        .and_then(|i| args.get(i + 1))
        .cloned();
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
            Some("tools/list") => Ok(json!({"tools": tools()})),
            Some("tools/call") => match call(&message["params"]) {
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

fn tools() -> Value {
    json!([
        {"name": "echo", "description": "Says back what it is given; {max_lines} stays as written.",
         "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
        {"name": "env", "description": "Says whether crowbot's key and FAKE_TOKEN reached it.",
         "inputSchema": {"type": "object"}},
        {"name": "fail", "description": "Always fails.", "inputSchema": {"type": "object"}},
        {"name": "picture", "description": "Answers with an image.",
         "inputSchema": {"type": "object"}},
        {"name": "look", "description": "Looks without touching anything.",
         "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}},
        {"name": "slow", "description": "Never answers.", "inputSchema": {"type": "object"}},
        {"name": "die", "description": "Exits at once.", "inputSchema": {"type": "object"}}
    ])
}

enum Call {
    Answer(Value),
    /// Left unanswered, as a tool still at work.
    Never,
    Unknown,
}

fn call(params: &Value) -> Call {
    let text = |text: String| Call::Answer(json!({"content": [{"type": "text", "text": text}]}));
    let seen = |name: &str| std::env::var(name).unwrap_or_else(|_| "unset".into());
    match params["name"].as_str().unwrap_or_default() {
        "echo" => text(
            params["arguments"]["text"]
                .as_str()
                .unwrap_or_default()
                .into(),
        ),
        "env" => text(format!(
            "key: {}; FAKE_TOKEN: {}",
            seen("CROWBOT_API_KEY"),
            seen("FAKE_TOKEN")
        )),
        "fail" => Call::Answer(json!({
            "content": [{"type": "text", "text": "fake-mcp: this tool always fails"}],
            "isError": true
        })),
        "picture" => Call::Answer(json!({
            "content": [{"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}]
        })),
        "look" => text("looked, changed nothing".into()),
        "slow" => Call::Never,
        "die" => {
            eprintln!("fake-mcp: dying as asked");
            std::process::exit(3);
        }
        _ => Call::Unknown,
    }
}
