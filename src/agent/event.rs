//! What a run reports as it goes. Serializable so `--json` is a plain dump of these.

use serde::Serialize;

use crate::agent::message::Assistant;
use crate::api::assemble::Delta;
use crate::api::error::ErrorInfo;

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Delta {
        kind: DeltaKind,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Retry {
        attempt: u32,
        delay_ms: u64,
        error: ErrorInfo,
    },
    MessageEnd {
        message: Assistant,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Text,
    Reasoning,
    ToolCall,
}

impl From<Delta<'_>> for AgentEvent {
    fn from(delta: Delta<'_>) -> Self {
        let (kind, text, index, name) = match delta {
            Delta::Text(t) => (DeltaKind::Text, t, None, None),
            Delta::Reasoning(t) => (DeltaKind::Reasoning, t, None, None),
            Delta::ToolCall {
                index,
                name,
                arguments,
            } => (
                DeltaKind::ToolCall,
                arguments,
                Some(index),
                Some(name.to_owned()),
            ),
        };
        Self::Delta {
            kind,
            text: text.to_owned(),
            index,
            name,
        }
    }
}
