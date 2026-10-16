//! An MCP tool's answer as the model reads it: its text, a note standing for anything else, cut
//! to the tool limits.
// CEILING: text only; images and audio are named, not sent. Upgrade: pass an image on as a part of
// the chat request once crowbot sends any.

use serde_json::Value;

use super::TEXT;
use crate::mcp::protocol::CallResult;
use crate::text::template::fill;
use crate::tools::Output;

pub fn output(result: CallResult) -> Output {
    let mut text = result
        .content
        .iter()
        .map(part)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty()
        && let Some(structured) = &result.structured
    {
        text = structured.to_string();
    }
    if text.trim().is_empty() {
        text = TEXT.no_output.clone();
    }
    let mut out = Output::head(&text);
    out.is_error = result.is_error;
    out
}

fn part(item: &Value) -> String {
    let field = |key: &str| item[key].as_str().unwrap_or_default();
    match field("type") {
        "text" => field("text").to_owned(),
        kind @ ("image" | "audio") => {
            fill(&TEXT.media, &[("kind", kind), ("mime", field("mimeType"))])
        }
        "resource" => match item["resource"]["text"].as_str() {
            Some(text) => text.to_owned(),
            None => {
                let uri = item["resource"]["uri"].as_str().unwrap_or_default();
                fill(&TEXT.resource, &[("uri", uri)])
            }
        },
        "resource_link" => fill(
            &TEXT.link,
            &[("name", field("name")), ("uri", field("uri"))],
        ),
        kind => fill(&TEXT.other, &[("kind", kind)]),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::config::limits;

    fn answer(result: Value) -> Output {
        output(serde_json::from_value(result).unwrap())
    }

    #[test]
    fn text_is_kept_and_what_is_not_text_is_named() {
        let out = answer(json!({"content": [
            {"type": "text", "text": "one"},
            {"type": "image", "data": "iVBOR", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "inside"}},
            {"type": "resource", "resource": {"uri": "file:///b.bin", "blob": "AAA"}},
            {"type": "resource_link", "uri": "file:///c", "name": "c"},
            {"type": "hologram"},
        ]}));
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines[0], "one");
        assert!(
            lines[1].starts_with("[image (image/png) not shown"),
            "{}",
            lines[1]
        );
        assert!(!out.content.contains("iVBOR"));
        assert_eq!(lines[2], "inside");
        assert!(lines[3].contains("file:///b.bin"), "{}", lines[3]);
        assert_eq!(lines[4], "[link c: file:///c]");
        assert!(lines[5].starts_with("[hologram content"), "{}", lines[5]);
        assert!(!out.is_error);
    }

    #[test]
    fn a_tool_that_failed_says_so_and_an_empty_answer_says_nothing_came() {
        let failed =
            answer(json!({"content": [{"type": "text", "text": "no such repo"}], "isError": true}));
        assert!(failed.is_error);
        assert_eq!(failed.content, "no such repo");
        assert_eq!(answer(json!({"content": []})).content, TEXT.no_output);
        let structured = answer(json!({"content": [], "structuredContent": {"n": 1}}));
        assert_eq!(structured.content, r#"{"n":1}"#);
    }

    #[test]
    fn a_long_answer_is_cut_like_any_tool_output() {
        let max = limits::get().tools.max_lines.value;
        let text: String = (0..max + 5).map(|i| format!("{i}\n")).collect();
        let out = answer(json!({"content": [{"type": "text", "text": text}]}));
        assert_eq!(
            out.content
                .lines()
                .filter(|l| l.parse::<usize>().is_ok())
                .count(),
            max
        );
    }
}
