//! A stand-in MCP server for crowbot's e2e suite: JSON-RPC over stdin and stdout, one message a
//! line. `--exit` fails at once, saying so on stderr.
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
         "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}
    ])
}

fn reply(out: &mut impl Write, message: Value) {
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}
