//! Tool calls as transcript cards, all drawn by this one renderer from data/tool_cards.toml.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

use crate::agent::message::ToolResult;
use crate::limits;
use crate::text::diff;
use crate::text::styled::{Line, Style};
use crate::tui::boxed::capped;

const SRC: &str = include_str!("../../data/tool_cards.toml");

static CARDS: LazyLock<BTreeMap<String, Spec>> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/tool_cards.toml is checked by tests"));

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    #[serde(default)]
    title: Option<Vec<String>>,
    #[serde(default)]
    body: Option<Body>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    max_lines: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Body {
    None,
    Diff,
    Head,
    Tail,
    Summary,
}

/// Where a call is in its life, which colours its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State<'a> {
    /// Running; carries the spinner frame to show.
    Running(&'a str),
    Done,
    Failed,
}

/// A tool's section merged over `[default]`.
fn spec(tool: &str) -> Spec {
    let default = CARDS.get("default").cloned().unwrap_or_default();
    let own = CARDS.get(tool).cloned().unwrap_or_default();
    Spec {
        title: own.title.or(default.title),
        body: own.body.or(default.body),
        summary: own.summary.or(default.summary),
        max_lines: own.max_lines.or(default.max_lines),
    }
}

pub fn header(tool: &str, arguments: &str, state: State<'_>) -> Line {
    let (mark, role) = match state {
        State::Running(frame) => (frame, "running"),
        State::Done => ("●", "done"),
        State::Failed => ("●", "error"),
    };
    let mut line = Line::styled(format!("{mark} "), Style::fg(role));
    line.push(tool, Style::default().bold());
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let title = spec(tool)
        .title
        .unwrap_or_default()
        .iter()
        .find_map(|key| args[key].as_str().map(str::to_owned));
    if let Some(title) = title {
        line.push(" ", Style::default());
        line.push(title.lines().next().unwrap_or_default(), Style::fg("muted"));
    }
    line
}

/// The finished card: header, then the body its spec asks for, indented under it.
pub fn finished(tool: &str, arguments: &str, result: &ToolResult, width: usize) -> Vec<Line> {
    let state = if result.is_error {
        State::Failed
    } else {
        State::Done
    };
    let mut lines = vec![header(tool, arguments, state).truncate(width)];
    let inner = width.saturating_sub(2);
    let body = if result.is_error {
        let max = limits::get().tui.error_lines.value;
        result
            .content
            .lines()
            .take(max)
            .map(|l| Line::styled(l, Style::fg("error")).truncate(inner))
            .collect()
    } else {
        body(tool, result, inner)
    };
    lines.extend(body.into_iter().map(|l| {
        let mut indented = Line::plain("  ");
        indented.extend(l);
        indented
    }));
    lines
}

fn body(tool: &str, result: &ToolResult, width: usize) -> Vec<Line> {
    let spec = spec(tool);
    let max = spec.max_lines.unwrap_or(1);
    let dim = |l: &str| Line::styled(l, Style::default().dim()).truncate(width);
    let lines: Vec<&str> = result.content.lines().collect();
    match spec.body.unwrap_or(Body::None) {
        Body::None => Vec::new(),
        Body::Diff => {
            let text = result
                .details
                .as_ref()
                .and_then(|d| d["diff"].as_str())
                .unwrap_or_default();
            capped(diff::render(text, width), max)
        }
        Body::Head => lines.iter().take(max).map(|l| dim(l)).collect(),
        Body::Tail => {
            let skip = lines.len().saturating_sub(max);
            lines[skip..].iter().map(|l| dim(l)).collect()
        }
        Body::Summary => {
            let template = spec.summary.unwrap_or_default();
            vec![Line::styled(fill(&template, result), Style::fg("muted")).truncate(width)]
        }
    }
}

/// `{a.b}` reads `details.a.b` (array indices too); `{lines}` counts output lines.
fn fill(template: &str, result: &ToolResult) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        out.push_str(&rest[..open]);
        let key = &rest[open + 1..open + close];
        out.push_str(&lookup(key, result));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

fn lookup(key: &str, result: &ToolResult) -> String {
    if key == "lines" {
        return result.content.lines().count().to_string();
    }
    let mut value = result.details.clone().unwrap_or(Value::Null);
    for part in key.split('.') {
        value = match part.parse::<usize>() {
            Ok(i) => value[i].clone(),
            Err(_) => value[part].clone(),
        };
    }
    match value {
        Value::String(s) => s,
        Value::Null => "?".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn result(content: &str, details: Option<Value>, is_error: bool) -> ToolResult {
        ToolResult {
            call_id: "c".into(),
            name: "x".into(),
            content: content.into(),
            is_error,
            details,
        }
    }

    fn tagged(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::to_tagged).collect()
    }

    #[test]
    fn every_section_names_a_real_tool() {
        let app = crate::tools::testing::Project::new();
        let registry = crate::tools::Registry::builtin(&app.app);
        let names = registry.names();
        for tool in CARDS.keys().filter(|k| *k != "default") {
            assert!(
                names.split(", ").any(|n| n == tool),
                "{tool} in data/tool_cards.toml is not a tool"
            );
        }
    }

    #[test]
    fn each_body_kind() {
        let read = finished(
            "read",
            r#"{"path":"src/a.rs"}"#,
            &result("x", Some(json!({"lines": [1, 40], "total": 90})), false),
            60,
        );
        assert_eq!(
            tagged(&read),
            vec![
                "[done]● [/][bold]read[/] [muted]src/a.rs[/]",
                "  [muted]lines 1-40 of 90[/]"
            ]
        );
        let bash = finished(
            "bash",
            r#"{"command":"cargo test"}"#,
            &result(
                &(1..=10).map(|i| format!("l{i}\n")).collect::<String>(),
                None,
                false,
            ),
            60,
        );
        assert_eq!(bash.len(), 1 + 8);
        assert!(bash[8].text().ends_with("l10"));
        let edit = finished(
            "edit",
            r#"{"path":"a"}"#,
            &result("ok", Some(json!({"diff": "@@ -1 +1 @@\n-a\n+b\n"})), false),
            60,
        );
        assert_eq!(edit[2].to_tagged(), "  [diff_del]-a[/]");
    }

    #[test]
    fn failures_show_the_start_of_the_error() {
        let card = finished(
            "bash",
            r#"{"command":"x"}"#,
            &result("boom\nmore", None, true),
            60,
        );
        assert_eq!(
            tagged(&card),
            vec![
                "[error]● [/][bold]bash[/] [muted]x[/]",
                "  [error]boom[/]",
                "  [error]more[/]"
            ]
        );
    }

    #[test]
    fn running_headers_show_the_spinner() {
        let line = header("grep", r#"{"pattern":"fn main"}"#, State::Running("⠋"));
        assert_eq!(
            line.to_tagged(),
            "[running]⠋ [/][bold]grep[/] [muted]fn main[/]"
        );
    }
}
