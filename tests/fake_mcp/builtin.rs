//! The fixed tools the e2e suite's scenarios call, each standing in for one way a server behaves.

use serde_json::{Value, json};

use crate::Call;

pub fn tools() -> Vec<Value> {
    vec![
        json!({"name": "echo", "description": "Says back what it is given; {max_lines} stays as written.",
         "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}),
        json!({"name": "env", "description": "Says whether crowbot's key and FAKE_TOKEN reached it.",
         "inputSchema": {"type": "object"}}),
        json!({"name": "fail", "description": "Always fails.", "inputSchema": {"type": "object"}}),
        json!({"name": "picture", "description": "Answers with an image.",
         "inputSchema": {"type": "object"}}),
        json!({"name": "look", "description": "Looks without touching anything.",
         "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}),
        json!({"name": "slow", "description": "Never answers.", "inputSchema": {"type": "object"}}),
        json!({"name": "die", "description": "Exits at once.", "inputSchema": {"type": "object"}}),
    ]
}

pub fn call(params: &Value) -> Call {
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
