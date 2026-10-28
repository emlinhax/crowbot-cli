//! What a run reports as it goes. `--json` prints each as one line; tags and field names are
//! stable, and changes are additions (tests/golden/json pins them).
//! CEILING: no version in the stream; a breaking change adds a leading `{"type":"start","v":N}`.

use serde::Serialize;

use crate::api::assemble::Delta;
use crate::api::chat::Event;
use crate::conversation::error::ErrorInfo;
use crate::conversation::message::{Assistant, ToolResult, Usage};
use crate::live::prompt::Prompt;

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
    /// A subagent hit its turn limit in data/limits.toml.
    TurnLimit,
}

/// How a run ended, and after how many model turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Ended {
    pub outcome: Outcome,
    pub turns: u32,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Every tool call in a `MessageEnd` gets one `ToolStart` and one `ToolEnd`, refused ones
    /// included, in call order.
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
    /// Earlier turns are being summarised; `auto` when the window filled, `overflow` when
    /// crowbot refused a request as too long.
    CompactionStart {
        auto: bool,
        overflow: bool,
    },
    /// The summary is in: `summarised` messages it stands for, `kept` word for word after it.
    /// Nothing summarised and no error means there was nothing to summarise yet.
    CompactionEnd {
        summarised: usize,
        kept: usize,
        usage: Option<Usage>,
        error: Option<String>,
    },
    /// What a subagent's run said, started by the `task` call `call_id`; a subagent's own
    /// subagents nest.
    Child {
        call_id: String,
        event: Box<AgentEvent>,
    },
    /// What is said once, at the start, of an MCP server config.toml names (it is not running,
    /// a tool of its is left out): the server, and the whole sentence.
    Mcp {
        server: String,
        note: String,
    },
    /// Snapshots for /undo stopped working; the session goes on without them.
    SnapshotOff {
        why: String,
    },
    /// A message typed while the run was going has just been handed to the model.
    Delivered {
        text: String,
    },
    RunEnd(Ended),
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

impl From<Event<'_>> for AgentEvent {
    fn from(event: Event<'_>) -> Self {
        match event {
            Event::Delta(delta) => delta.into(),
            Event::Retry {
                attempt,
                delay,
                error,
            } => Self::Retry {
                attempt,
                delay_ms: delay.as_millis() as u64,
                error: error.clone(),
            },
        }
    }
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

impl AgentEvent {
    /// What the run that said it said, however deep the subagent.
    pub fn innermost(&self) -> &AgentEvent {
        match self {
            Self::Child { event, .. } => event.innermost(),
            event => event,
        }
    }
}
