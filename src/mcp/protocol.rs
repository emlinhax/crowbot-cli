//! The MCP messages crowbot sends and reads, as far as it reads them.

use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::data;
use crate::release;

pub static DATA: LazyLock<Data> = data::load!("mcp.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub protocol: String,
    pub accepted: Vec<String>,
    pub client: String,
    pub text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub not_found: String,
    pub no_start: String,
    pub timed_out: String,
    pub version: String,
    pub closed: String,
    pub said: String,
    pub not_running: String,
    pub stopped: String,
    pub http_status: String,
    pub too_long: String,
}

/// What `initialize` asks with: the version crowbot speaks, no capabilities of its own.
pub fn initialize() -> Value {
    json!({
        "protocolVersion": DATA.protocol,
        "capabilities": {},
        "clientInfo": {"name": DATA.client, "version": release::shown()},
    })
}

#[derive(Deserialize)]
pub struct Initialized {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// How to use its tools, for the model.
    #[serde(default)]
    pub instructions: Option<String>,
}

/// One page of `tools/list`.
#[derive(Deserialize)]
pub struct ToolPage {
    #[serde(default)]
    pub tools: Vec<ToolInfo>,
    #[serde(rename = "nextCursor", default)]
    pub next_cursor: Option<String>,
}

/// A tool as its server describes it.
#[derive(Clone, Debug, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// A JSON Schema of its arguments.
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Annotations,
}

/// What a server says of how a tool behaves; hints, not promises.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Annotations {
    /// It changes nothing.
    #[serde(rename = "readOnlyHint", default)]
    pub read_only: bool,
}

/// What `tools/call` asks with.
pub fn call(tool: &str, arguments: Value) -> Value {
    json!({"name": tool, "arguments": arguments})
}

/// A tool's answer: its content, and whether the tool itself failed.
#[derive(Debug, Deserialize)]
pub struct CallResult {
    #[serde(default)]
    pub content: Vec<Value>,
    /// The same answer as JSON, from a tool that declares an output schema.
    #[serde(rename = "structuredContent", default)]
    pub structured: Option<Value>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}
