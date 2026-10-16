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
}

/// One page of `tools/list`; each tool as the server describes it.
#[derive(Deserialize)]
pub struct ToolPage {
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(rename = "nextCursor", default)]
    pub next_cursor: Option<String>,
}
