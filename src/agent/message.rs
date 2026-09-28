//! The conversation as crowbot stores it. Serialized into session files, so every field that can
//! be absent carries `#[serde(default)]` and new variants are additions, never renames.

use serde::{Deserialize, Serialize};

use crate::api::error::ErrorInfo;

// Messages are few and rarely moved; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User { parts: Vec<Part> },
    Assistant(Assistant),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    /// The model's thinking trace; crowbot requires it echoed back on later turns.
    Reasoning {
        text: String,
    },
    ToolCall(ToolCall),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON text exactly as the model produced it, echoed back byte for byte.
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assistant {
    pub parts: Vec<Part>,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    pub finish: Finish,
    #[serde(default)]
    pub error: Option<ErrorInfo>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// The model finished its reply.
    Done,
    ToolCalls,
    /// Output hit `max_tokens`; on crowbot a low balance can lower that cap silently.
    Length,
    Error,
    Aborted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens including the cached ones.
    pub input: u64,
    #[serde(default)]
    pub cached: u64,
    pub output: u64,
    #[serde(default)]
    pub reasoning: u64,
    /// What this request cost at the model's listed rates, in millionths of a dollar.
    #[serde(default)]
    pub cost_micros: u64,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            parts: vec![Part::Text { text: text.into() }],
        }
    }
}
