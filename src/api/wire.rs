//! The OpenAI chat-completions shapes crowbot speaks, and the mapping from our messages to them.

use serde::{Deserialize, Serialize};

use crate::agent::message::{Message, Part};
use crate::api::error::ErrorInfo;

#[derive(Debug, Serialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    pub stream: bool,
    pub stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum WireMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall>,
    },
}

#[derive(Debug, PartialEq, Serialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

/// How assistant turns are replayed to the model they are sent to.
pub struct Replay<'a> {
    pub model: &'a str,
    /// Reasoning-capable models reject assistant turns without `reasoning_content`, even empty.
    pub reasoning: bool,
}

impl WireMessage {
    pub fn from_message(message: &Message, replay: &Replay) -> Self {
        match message {
            Message::User { parts } => Self::User {
                content: parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            },
            Message::Assistant(a) => {
                let mut text = String::new();
                let mut reasoning = String::new();
                let mut tool_calls = Vec::new();
                for part in &a.parts {
                    match part {
                        Part::Text { text: t } => text.push_str(t),
                        Part::Reasoning { text: r } => reasoning.push_str(r),
                        Part::ToolCall(call) => tool_calls.push(WireToolCall {
                            id: call.id.clone(),
                            kind: "function",
                            function: WireFunction {
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                            },
                        }),
                    }
                }
                // A trace only means something to the model that wrote it.
                let echo = a.model == replay.model && !reasoning.is_empty();
                let reasoning_content = match (echo, replay.reasoning) {
                    (true, _) => Some(reasoning),
                    (false, true) => Some(String::new()),
                    (false, false) => None,
                };
                Self::Assistant {
                    content: (!text.is_empty() || tool_calls.is_empty()).then_some(text),
                    reasoning_content,
                    tool_calls,
                }
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Chunk {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
    /// A failure after the stream opened arrives as this envelope in a data frame.
    #[serde(default)]
    pub error: Option<ErrorInfo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct WireUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionDetails>,
    /// DeepSeek reports cache hits here.
    #[serde(default)]
    pub prompt_cache_hit_tokens: Option<u64>,
    /// Kimi reports cache hits here.
    #[serde(default)]
    pub cached_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PromptDetails {
    #[serde(default)]
    pub cached_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CompletionDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}

impl WireUsage {
    /// crowbot resells several vendors, each reporting cache hits in its own place.
    pub fn cached(&self) -> u64 {
        self.prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .or(self.prompt_cache_hit_tokens)
            .or(self.cached_tokens)
            .unwrap_or(0)
    }

    pub fn reasoning(&self) -> u64 {
        self.completion_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Assistant, Finish, ToolCall};

    fn assistant(model: &str, parts: Vec<Part>) -> Message {
        Message::Assistant(Assistant {
            parts,
            model: model.into(),
            effort: None,
            usage: None,
            finish: Finish::Done,
            error: None,
            request_id: None,
        })
    }

    #[test]
    fn reasoning_is_echoed_only_to_the_model_that_wrote_it() {
        let msg = assistant(
            "a",
            vec![
                Part::Reasoning {
                    text: "think".into(),
                },
                Part::Text { text: "hi".into() },
            ],
        );
        let same = WireMessage::from_message(
            &msg,
            &Replay {
                model: "a",
                reasoning: true,
            },
        );
        let other = WireMessage::from_message(
            &msg,
            &Replay {
                model: "b",
                reasoning: true,
            },
        );
        let plain = WireMessage::from_message(
            &msg,
            &Replay {
                model: "b",
                reasoning: false,
            },
        );
        let reasoning = |m: WireMessage| match m {
            WireMessage::Assistant {
                reasoning_content, ..
            } => reasoning_content,
            _ => panic!("assistant expected"),
        };
        assert_eq!(reasoning(same).as_deref(), Some("think"));
        assert_eq!(reasoning(other).as_deref(), Some(""));
        assert_eq!(reasoning(plain), None);
    }

    #[test]
    fn tool_calls_keep_raw_arguments_and_omit_empty_content() {
        let msg = assistant(
            "a",
            vec![Part::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: "{\"path\": \"a.rs\"}".into(),
            })],
        );
        let json = serde_json::to_value(WireMessage::from_message(
            &msg,
            &Replay {
                model: "a",
                reasoning: false,
            },
        ))
        .unwrap();
        assert_eq!(json["role"], "assistant");
        assert!(json.get("content").is_none());
        assert_eq!(
            json["tool_calls"][0]["function"]["arguments"],
            "{\"path\": \"a.rs\"}"
        );
        assert_eq!(json["tool_calls"][0]["type"], "function");
    }

    #[test]
    fn cached_tokens_are_read_from_every_vendor_shape() {
        let parse = |s: &str| serde_json::from_str::<WireUsage>(s).unwrap().cached();
        assert_eq!(parse(r#"{"prompt_tokens_details":{"cached_tokens":5}}"#), 5);
        assert_eq!(parse(r#"{"prompt_cache_hit_tokens":6}"#), 6);
        assert_eq!(parse(r#"{"cached_tokens":7}"#), 7);
        assert_eq!(parse(r#"{}"#), 0);
    }
}
