//! What a run reports as it goes. `--json` prints each as one line; tags and field names are
//! stable, and changes are additions (tests/golden/json pins them).
//! CEILING: no version in the stream; a breaking change adds a leading `{"type":"start","v":N}`.

use serde::Serialize;

use crate::agent::message::{Assistant, ToolResult};
use crate::agent::prompt::Prompt;
use crate::api::assemble::Delta;
use crate::api::error::ErrorInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The model finished and nothing was queued.
    Done,
    Aborted,
    /// The last reply ended in an error.
    Failed,
    /// The user turned a call down without saying why.
    Rejected,
    /// The run hit the turn limit in data/limits.toml.
    TurnLimit,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    ToolStart {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolEnd {
        result: ToolResult,
    },
    /// A call waits on the user; answer through `Shared::answer` with this id.
    Prompt {
        id: u64,
        call_id: String,
        prompt: Prompt,
    },
    /// The session file stopped being written; the run goes on, unsaved from here.
    Unsaved {
        path: String,
        error: String,
    },
    /// A message typed while the run was going has just been handed to the model.
    Delivered {
        text: String,
    },
    RunEnd {
        outcome: Outcome,
    },
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
