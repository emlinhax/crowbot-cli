//! The conversation as crowbot stores it. Serialized into session files, so every field that can
//! be absent carries `#[serde(default)]` and new variants are additions, never renames.

use serde::{Deserialize, Serialize};

use crate::conversation::error::ErrorInfo;
use crate::permission::gate::Ask;

// Messages are few and rarely moved; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User { parts: Vec<Part> },
    Assistant(Assistant),
    Tool(ToolResult),
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
    /// Context crowbot adds for the model (mode changes and the like); not the user's words.
    Reminder {
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON text exactly as the model produced it, echoed back byte for byte.
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    /// What the model sees.
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
    /// For display only (diffs, exit codes, spill paths); never sent to the model.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    /// What the call needed permission for, as the gate saw it; recorded, never sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<Ask>,
    /// Why the call did not run: the data/model_text.toml key of what the model was told, or
    /// `check` when the tool's own check refused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// What answering it cost, for a call that ran requests of its own (a subagent's);
    /// recorded, never sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
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

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cost_micros += other.cost_micros;
    }
}

impl Message {
    #[cfg(test)]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            parts: vec![Part::Text { text: text.into() }],
        }
    }
}

impl Part {
    /// The text parts of `parts`, a line apart; reasoning, calls and reminders are left out.
    pub fn text_of(parts: &[Part]) -> String {
        parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Assistant {
    /// A finished reply from model `m` with nothing else set, for tests to build on.
    #[cfg(test)]
    pub fn stub(finish: Finish, parts: Vec<Part>) -> Self {
        Self {
            parts,
            model: "m".into(),
            effort: None,
            usage: None,
            finish,
            error: None,
            request_id: None,
        }
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.parts.iter().filter_map(|p| match p {
            Part::ToolCall(call) => Some(call),
            _ => None,
        })
    }
}
