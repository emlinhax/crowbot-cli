//! A stand-in MCP server for crowbot's e2e suite: JSON-RPC over stdin and stdout, one message a
//! line, offering the tools in `tools()`. `--exit` fails at once, saying so on stderr.
// A test program talking on stdin and stdout; the io-wrapper rule is for crowbot itself.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--exit") {
        eprintln!("fake-mcp: told to fail at start");
        std::process::exit(1);
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let result = match message["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake", "version": "0"}
            }),
            Some("tools/list") => json!({"tools": tools()}),
            Some("tools/call") => match call(&message["params"]) {
                Some(result) => result,
                None => {
                    let error = json!({"code": -32602, "message": "no such tool"});
                    reply(
                        &mut out,
                        json!({"jsonrpc": "2.0", "id": id, "error": error}),
                    );
                    continue;
                }
            },
            _ => {
                let error = json!({"code": -32601, "message": "no such method"});
                reply(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "error": error}),
                );
                continue;
            }
        };
        reply(
            &mut out,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        );
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
         "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}
    ])
}

/// The answer to `tools/call`; `None` for a tool it does not offer.
fn call(params: &Value) -> Option<Value> {
    let text = |text: String| json!({"content": [{"type": "text", "text": text}]});
    let seen = |name: &str| std::env::var(name).unwrap_or_else(|_| "unset".into());
    Some(match params["name"].as_str()? {
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
        "fail" => json!({"content": [{"type": "text", "text": "fake-mcp: this tool always fails"}],
                         "isError": true}),
        "look" => text("looked, changed nothing".into()),
        "picture" => json!({"content": [{"type": "image", "data": "iVBORw0KGgo=",
                                         "mimeType": "image/png"}]}),
        _ => return None,
    })
}

fn reply(out: &mut impl Write, message: Value) {
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}
