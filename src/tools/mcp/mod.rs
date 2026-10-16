//! An MCP server's tools as crowbot's: named `<server>__<tool>`, described as the server words
//! them, asking `mcp` and `mcp_write` by that name, and answered with the server's reply as text.

mod result;

use std::sync::{Arc, LazyLock};

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Check, Output, Refusal, Registry, Spec, Tool, ToolCx};
use crate::config::limits;
use crate::data;
use crate::mcp::Servers;
use crate::mcp::protocol::ToolInfo;
use crate::mcp::server::Caller;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = data::load!("tools/mcp.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    not_object: String,
    failed: String,
    no_output: String,
    media: String,
    resource: String,
    link: String,
    other: String,
    left_out: String,
}

pub struct McpTool {
    spec: Spec,
    server: String,
    /// The name its server knows it by.
    tool: String,
    caller: Caller,
}

/// Adds every running server's tools to `registry` after the others, servers in name order, so
/// each request lists them alike; one whose name another tool has is left out, said at the start.
pub fn join(servers: &mut Servers, registry: &mut Registry) {
    let mut left_out = Vec::new();
    for server in servers.running() {
        for info in &server.tools {
            let tool = McpTool::new(&server.name, info, server.caller());
            let name = tool.spec.name.clone();
            if registry.add(Arc::new(tool), &[]).is_err() {
                let vars = [
                    ("server", server.name.as_str()),
                    ("tool", &info.name),
                    ("name", &name),
                ];
                left_out.push((server.name.clone(), fill(&TEXT.left_out, &vars)));
            }
        }
    }
    for (server, note) in left_out {
        servers.note(&server, note);
    }
}

impl McpTool {
    fn new(server: &str, info: &ToolInfo, caller: Caller) -> Self {
        Self {
            spec: Spec {
                name: name(server, &info.name),
                description: info.description.clone(),
                parameters: parameters(&info.input_schema),
            },
            server: server.to_owned(),
            tool: info.name.clone(),
            caller,
        }
    }
}

/// `<server>__<tool>` in the characters a tool's name may hold, the tool's part cut so the whole
/// fits `mcp.name_chars`; the server's name is checked to fit already.
fn name(server: &str, tool: &str) -> String {
    let room = limits::get()
        .mcp
        .name_chars
        .value
        .saturating_sub(server.len() + 2);
    let tool: String = tool
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                true => c,
                false => '_',
            },
        )
        .take(room)
        .collect();
    format!("{server}__{tool}")
}

/// The server's schema of the arguments as an object schema, which is all a chat request takes.
// CEILING: the schema goes out as the server wrote it, so one a vendor refuses fails every
// request. Upgrade: check each against what the vendors take and leave out a tool that fails.
fn parameters(schema: &Value) -> Value {
    let mut schema = match schema {
        Value::Object(_) => schema.clone(),
        _ => json!({}),
    };
    // A tool without arguments may say no more than that; some vendors refuse an object schema
    // without `type` or `properties`.
    if let Some(object) = schema.as_object_mut() {
        object.entry("type").or_insert_with(|| json!("object"));
        object.entry("properties").or_insert_with(|| json!({}));
    }
    schema
}

impl Tool for McpTool {
    type Args = Value;

    fn spec(&self) -> &Spec {
        &self.spec
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if !args.is_object() {
            return Err(Refusal::InvalidArgs(TEXT.not_object.clone()));
        }
        let name = &self.spec.name;
        let asks = vec![
            Ask::new(catalog::MCP.name, name.clone()),
            Ask::new(catalog::MCP_WRITE.name, name.clone()),
        ];
        let preview = serde_json::to_string_pretty(args).unwrap_or_default();
        Ok(Check::new(asks).with_preview(preview))
    }

    fn run<'a>(
        &'a self,
        args: Value,
        _cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let answer =
                self.caller.call(&self.tool, args).await.map_err(|why| {
                    fill(&TEXT.failed, &[("server", &self.server), ("why", &why)])
                })?;
            Ok(result::output(answer))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::default_rules;
    use crate::mcp::server::playing;
    use crate::mode;
    use crate::permission::gate::Decision;
    use crate::permission::rule::{Action, Rule};
    use crate::tools::testing::Project;

    fn info(name: &str, schema: Value) -> ToolInfo {
        serde_json::from_value(json!({"name": name, "description": "d", "inputSchema": schema}))
            .unwrap()
    }

    fn echo() -> McpTool {
        let caller = playing(|method, params| {
            (method == "tools/call").then(|| {
                let said = params["arguments"]["text"].as_str().unwrap_or_default();
                json!({"content": [{"type": "text", "text": format!("{}: {said}", params["name"])}]})
            })
        });
        McpTool::new("fake", &info("echo", json!({"type": "object"})), caller)
    }

    #[test]
    fn names_join_server_and_tool_in_the_characters_a_name_may_hold_cut_to_fit() {
        assert_eq!(name("github", "get_issue"), "github__get_issue");
        assert_eq!(name("fs", "read.file v2"), "fs__read_file_v2");
        let long = name("server", &"x".repeat(100));
        assert_eq!(long.len(), limits::get().mcp.name_chars.value);
        assert!(long.starts_with("server__xxx"), "{long}");
    }

    #[test]
    fn a_schema_goes_out_as_written_with_what_an_object_schema_needs() {
        let full =
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]});
        assert_eq!(parameters(&full), full);
        assert_eq!(
            parameters(&Value::Null),
            json!({"type": "object", "properties": {}})
        );
        let bare = parameters(&json!({"additionalProperties": false}));
        assert_eq!(bare["type"], "object");
        assert_eq!(bare["additionalProperties"], false);
    }

    #[tokio::test]
    async fn a_call_reaches_the_server_by_its_own_name_and_its_text_comes_back() {
        let tool = echo();
        assert_eq!(tool.spec.name, "fake__echo");
        assert_eq!(tool.spec.description, "d");
        let out = Project::new().run(&tool, json!({"text": "hi"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "\"echo\": hi");
    }

    #[tokio::test]
    async fn a_server_that_does_not_answer_fails_the_call_naming_it() {
        let caller = playing(|_, _| None);
        let tool = McpTool::new("slow", &info("wait", Value::Null), caller);
        let out = Project::new().run(&tool, json!({})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("MCP server slow: "),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn manual_asks_auto_runs_plan_refuses_and_a_rule_can_allow_a_server() {
        let project = Project::new();
        let check = project.check(&echo(), &json!({"text": "hi"})).unwrap();
        assert!(project.check(&echo(), &json!("hi")).is_err());
        let decide = |mode: &str, rules: &[Rule]| {
            let mode = mode::get(mode).unwrap();
            let locks = mode.locks("/home/u/.crowbot/plans/s1.md");
            mode.policy(rules, &locks).decide(&check.asks)
        };
        let mut rules = default_rules();
        assert_eq!(decide("manual", &rules), Decision::Ask);
        assert_eq!(decide("auto", &rules), Decision::Allow);
        assert!(matches!(decide("plan", &rules), Decision::Deny(_)));
        rules.push(Rule {
            permission: "mcp".into(),
            pattern: "fake__*".into(),
            action: Action::Allow,
        });
        assert_eq!(decide("manual", &rules), Decision::Allow);
        assert!(matches!(decide("plan", &rules), Decision::Deny(_)));
    }
}
