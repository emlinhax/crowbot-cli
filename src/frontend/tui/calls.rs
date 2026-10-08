//! Tool calls as transcript cards, all drawn by this one renderer from their specs
//! (frontend/tool_card.rs).

use serde_json::Value;

use crate::config::limits;
use crate::conversation::message::ToolResult;
use crate::frontend::tool_card::{self, Body, Summary, at, spec};
use crate::frontend::tui::rows::capped;
use crate::text::diff;
use crate::text::styled::{Line, Style};
use crate::text::template;

/// Where a call is in its life, which colours its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State<'a> {
    /// Running; carries the spinner frame to show.
    Running(&'a str),
    Done,
    Failed,
}

pub fn header(tool: &str, arguments: &str, state: State<'_>) -> Line {
    let (mark, role) = match state {
        State::Running(frame) => (frame, "running"),
        State::Done => ("●", "done"),
        State::Failed => ("●", "error"),
    };
    let mut line = Line::styled(format!("{mark} "), Style::fg(role));
    line.push(tool, Style::default().bold());
    if let Some(title) = tool_card::title(tool, arguments) {
        line.push(" ", Style::default());
        line.push(title, Style::fg("muted"));
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
    let max = spec.max_lines.unwrap_or_default();
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
            let text = spec
                .summary
                .and_then(|s| summarize(&s, result, false))
                .unwrap_or_default();
            vec![Line::styled(text, Style::fg("muted")).truncate(width)]
        }
    }
}

/// `{a.b}` reads `details.a.b` (array indices too).
/// A summary's line. Strict (inside a list) it is `None` unless every detail it names is there;
/// otherwise a missing one shows as `?`.
fn summarize(summary: &Summary, result: &ToolResult, strict: bool) -> Option<String> {
    let has = |key: &str| !at(result.details.as_ref().unwrap_or(&Value::Null), key).is_null();
    match summary {
        Summary::Text(text) => {
            let mut complete = true;
            template::fill_with(text, |key| {
                complete &= has(key);
                None
            });
            (complete || !strict).then(|| fill(text, result))
        }
        Summary::Count { count, forms } => {
            if strict && !has(count) {
                return None;
            }
            let n = lookup(count, result).parse().unwrap_or(0);
            Some(forms.of(n))
        }
        Summary::First(list) => list.iter().find_map(|s| summarize(s, result, true)),
    }
}

fn fill(template: &str, result: &ToolResult) -> String {
    template::fill_with(template, |key| Some(lookup(key, result).into()))
}

fn lookup(key: &str, result: &ToolResult) -> String {
    match at(result.details.as_ref().unwrap_or(&Value::Null), key) {
        Value::String(s) => s,
        Value::Null => "?".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_card_sets_every_number_a_card_falls_back_to() {
        let default = tool_card::default_spec();
        assert!(default.max_lines.is_some_and(|n| n > 0));
        assert!(default.title_items.is_some_and(|n| n > 1));
    }
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
        let registry = crate::tools::Registry::builtin(None);
        let names = registry.names();
        for tool in tool_card::tools() {
            assert!(
                names.split(", ").any(|n| n == tool),
                "{tool} in data/tool_cards.toml is not a tool"
            );
        }
    }

    #[test]
    fn a_forum_call_folds_onto_one_header_line() {
        let line = header(
            "forum_search",
            r#"{"forum":"unknowncheats","query":"aimbot"}"#,
            State::Done,
        );
        assert_eq!(
            line.to_tagged(),
            "[done]● [/][bold]forum_search[/] [muted]unknowncheats · aimbot[/]"
        );
        // A missing arg is skipped, not shown as a gap.
        let thread = header("forum_thread", r#"{"forum":"uc"}"#, State::Done);
        assert_eq!(
            thread.to_tagged(),
            "[done]● [/][bold]forum_thread[/] [muted]uc[/]"
        );
        // A batched call shows its ids on the one line.
        let batched = header(
            "forum_thread",
            r#"{"forum":"uc","topic":["318822","318823"]}"#,
            State::Done,
        );
        assert_eq!(
            batched.to_tagged(),
            "[done]● [/][bold]forum_thread[/] [muted]uc › 318822, 318823[/]"
        );
        // forum_search asks for one line only: no body.
        let card = finished(
            "forum_search",
            r#"{"forum":"uc","query":"x"}"#,
            &result("[42] a topic\n[43] another", None, false),
            60,
        );
        assert_eq!(card.len(), 1);
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
    fn counts_come_from_the_details_in_the_form_that_fits() {
        let summary = |tool: &str, details| {
            let card = finished(
                tool,
                r#"{"path":"a"}"#,
                &result("x", Some(details), false),
                60,
            );
            card[1].text()
        };
        assert_eq!(summary("write", json!({"lines": 1})).trim(), "1 line");
        assert_eq!(summary("write", json!({"lines": 120})).trim(), "120 lines");
        assert_eq!(summary("glob", json!({"files": 1})).trim(), "1 file");
        assert_eq!(summary("grep", json!({"matches": 0})).trim(), "0 matches");
        // A list shows the first summary whose details are all there.
        assert_eq!(summary("grep", json!({"files": 2})).trim(), "2 files");
        let read = json!({"lines": [1, 5], "total": 9});
        assert_eq!(summary("read", read).trim(), "lines 1-5 of 9");
        assert_eq!(summary("read", json!({"files": 3})).trim(), "3 files");
        assert_eq!(summary("read", json!({"entries": 4})).trim(), "4 entries");
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
