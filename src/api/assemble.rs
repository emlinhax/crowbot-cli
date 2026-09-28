//! Folds streamed chat-completion chunks into message parts, in the order they appeared.

use std::collections::BTreeMap;

use crate::agent::message::{Finish, Part, ToolCall};
use crate::api::error::ErrorInfo;
use crate::api::wire::{Chunk, WireUsage};

/// What just arrived, for live display.
#[derive(Debug, PartialEq)]
pub enum Delta<'a> {
    Text(&'a str),
    Reasoning(&'a str),
    ToolCall {
        index: usize,
        name: &'a str,
        arguments: &'a str,
    },
}

#[derive(Default)]
pub struct Assembler {
    parts: Vec<Part>,
    /// Tool-call index from the stream -> position in `parts`.
    tool_slots: BTreeMap<usize, usize>,
    usage: Option<WireUsage>,
    finish_reason: Option<String>,
    error: Option<ErrorInfo>,
    /// Whether any content reached the caller; a retry would then repeat billed output.
    pub emitted: bool,
}

pub struct Assembled {
    pub parts: Vec<Part>,
    pub finish: Finish,
    pub usage: Option<WireUsage>,
    pub error: Option<ErrorInfo>,
}

impl Assembler {
    pub fn apply(&mut self, chunk: Chunk, emit: &mut dyn FnMut(Delta<'_>)) {
        if chunk.usage.is_some() {
            self.usage = chunk.usage;
        }
        if chunk.error.is_some() {
            self.error = chunk.error;
        }
        for choice in chunk.choices {
            let delta = choice.delta;
            if let Some(text) = delta.reasoning_content.filter(|t| !t.is_empty()) {
                self.append(&text, true);
                emit(Delta::Reasoning(&text));
            }
            if let Some(text) = delta.content.filter(|t| !t.is_empty()) {
                self.append(&text, false);
                emit(Delta::Text(&text));
            }
            for fragment in delta.tool_calls {
                let slot = *self.tool_slots.entry(fragment.index).or_insert_with(|| {
                    self.parts.push(Part::ToolCall(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    }));
                    self.parts.len() - 1
                });
                let Part::ToolCall(call) = &mut self.parts[slot] else {
                    unreachable!("tool slots point at tool calls");
                };
                if let Some(id) = fragment.id.filter(|id| !id.is_empty()) {
                    call.id = id;
                }
                let function = fragment.function.unwrap_or_default();
                // Some vendors repeat the name on every fragment; the first one is the name.
                if call.name.is_empty()
                    && let Some(name) = function.name
                {
                    call.name = name;
                }
                let arguments = function.arguments.unwrap_or_default();
                call.arguments.push_str(&arguments);
                emit(Delta::ToolCall {
                    index: fragment.index,
                    name: &call.name,
                    arguments: &arguments,
                });
            }
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason;
            }
            self.emitted |= !self.parts.is_empty();
        }
    }

    /// Consumes the stream state; `cut` is why reading stopped early, if it did.
    pub fn finish(mut self, cut: Option<ErrorInfo>) -> Assembled {
        for (index, slot) in &self.tool_slots {
            if let Part::ToolCall(call) = &mut self.parts[*slot]
                && call.id.is_empty()
            {
                call.id = format!("call_{index}");
            }
        }
        let has_tools = !self.tool_slots.is_empty();
        let (finish, error) = match (self.error.or(cut), self.finish_reason.as_deref()) {
            (Some(error), _) => (Finish::Error, Some(error)),
            (None, Some("length")) => (Finish::Length, None),
            // Vendors disagree on whether a tool-calling turn finishes with "tool_calls" or "stop".
            (None, Some("tool_calls" | "stop")) if has_tools => (Finish::ToolCalls, None),
            (None, Some("stop")) => (Finish::Done, None),
            (None, Some(other)) => (
                Finish::Error,
                Some(ErrorInfo::local(
                    other,
                    format!("the model stopped: {other}"),
                )),
            ),
            (None, None) => (
                Finish::Error,
                Some(ErrorInfo::local(
                    "stream_ended",
                    "the stream ended before the model finished",
                )),
            ),
        };
        Assembled {
            parts: self.parts,
            finish,
            usage: self.usage,
            error,
        }
    }

    fn append(&mut self, text: &str, reasoning: bool) {
        match self.parts.last_mut() {
            Some(Part::Reasoning { text: t }) if reasoning => t.push_str(text),
            Some(Part::Text { text: t }) if !reasoning => t.push_str(text),
            _ if reasoning => self.parts.push(Part::Reasoning {
                text: text.to_owned(),
            }),
            _ => self.parts.push(Part::Text {
                text: text.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> (Assembled, Vec<String>) {
        let mut asm = Assembler::default();
        let mut seen = Vec::new();
        for c in chunks {
            asm.apply(serde_json::from_str(c).unwrap(), &mut |d| {
                seen.push(format!("{d:?}"))
            });
        }
        (asm.finish(None), seen)
    }

    #[test]
    fn interleaves_reasoning_and_text_in_order() {
        let (out, seen) = run(&[
            r#"{"choices":[{"delta":{"reasoning_content":"thin"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"king"}}]}"#,
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4}}"#,
        ]);
        assert_eq!(
            out.parts,
            vec![
                Part::Reasoning {
                    text: "thinking".into()
                },
                Part::Text {
                    text: "Hello".into()
                },
            ]
        );
        assert_eq!(out.finish, Finish::Done);
        assert_eq!(out.usage.unwrap().prompt_tokens, 10);
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn assembles_parallel_tool_calls_by_index() {
        let (out, _) = run(&[
            r#"{"choices":[{"delta":{"content":"Reading."}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"name":"glob","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":1}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(out.finish, Finish::ToolCalls);
        let calls: Vec<_> = out
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall(c) => Some((c.id.as_str(), c.name.as_str(), c.arguments.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![("a", "read", "{\"path\":1}"), ("call_1", "glob", "{}")]
        );
    }

    #[test]
    fn tool_calls_finishing_with_stop_still_count_as_tool_calls() {
        let (out, _) = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"x","arguments":"{}"}}]},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(out.finish, Finish::ToolCalls);
    }

    #[test]
    fn error_frame_wins_and_keeps_partial_text() {
        let (out, _) = run(&[
            r#"{"choices":[{"delta":{"content":"par"}}]}"#,
            r#"{"error":{"message":"upstream died","type":"upstream_error"}}"#,
        ]);
        assert_eq!(out.finish, Finish::Error);
        assert_eq!(out.error.unwrap().kind, "upstream_error");
        assert_eq!(out.parts, vec![Part::Text { text: "par".into() }]);
    }

    #[test]
    fn missing_finish_reason_is_a_cut_stream() {
        let (out, _) = run(&[r#"{"choices":[{"delta":{"content":"x"}}]}"#]);
        assert_eq!(out.error.unwrap().kind, "stream_ended");
    }

    #[test]
    fn length_and_filters_map_to_their_stops() {
        let (out, _) = run(&[r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#]);
        assert_eq!(out.finish, Finish::Length);
        let (out, _) = run(&[r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#]);
        assert_eq!(out.error.unwrap().kind, "content_filter");
    }
}
