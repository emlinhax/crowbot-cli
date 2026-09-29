//! Everything that waits on the user has one shape: a `Prompt` goes out as an event, and a `Reply`
//! comes back through `Shared::answer`. Headless runs answer `Unavailable` to all of them.

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::agent::event::AgentEvent;
use crate::agent::state::Shared;
use crate::permission::gate::Ask;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prompt {
    /// A tool call that needs the user's permission.
    Permission {
        tool: String,
        asks: Vec<Ask>,
        /// What the call would do: a diff, a command.
        #[serde(skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
    },
    /// The model asks the user to choose.
    Question {
        question: String,
        options: Vec<String>,
    },
    /// PLAN mode's plan is ready; the user decides what happens next.
    PlanExit { plan: String, choices: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Yes,
    /// With feedback the model hears why and carries on; without it a permission refusal stops
    /// the run.
    No {
        feedback: Option<String>,
    },
    /// An option by index.
    Choice(usize),
    /// A free-text answer ("Other…").
    Text(String),
    /// Nobody can answer (a headless run).
    Unavailable,
}

/// Asks and waits; `None` when the run was interrupted first.
pub async fn ask(
    shared: &Shared,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
    call_id: &str,
    prompt: Prompt,
    cancel: &CancellationToken,
) -> Option<Reply> {
    let (id, reply) = shared.register_ask(matches!(prompt, Prompt::Permission { .. }));
    emit(AgentEvent::Prompt {
        id,
        call_id: call_id.to_owned(),
        prompt,
    });
    tokio::select! {
        reply = reply => Some(reply.unwrap_or(Reply::Unavailable)),
        () = cancel.cancelled() => {
            shared.forget(id);
            None
        }
    }
}
