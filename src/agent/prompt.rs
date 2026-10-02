//! Everything that waits on the user has one shape: a `Prompt` goes out as an event, and a `Reply`
//! comes back through `Shared::answer`. Headless runs answer `Unavailable` to all of them.

use serde::{Deserialize, Serialize};
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
    /// The model asks the user to choose; several questions share one card.
    Question { questions: Vec<Question> },
    /// PLAN mode's plan is ready; the user decides what happens next.
    PlanExit { plan: String, choices: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    /// A word or two naming it on the card's tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<Offer>,
    /// More than one option may be picked.
    #[serde(default)]
    pub multiple: bool,
}

/// One option on a question. A bare string from the model is its label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "OfferIn")]
pub struct Offer {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OfferIn {
    Label(String),
    Full {
        label: String,
        #[serde(default)]
        description: Option<String>,
    },
}

impl From<OfferIn> for Offer {
    fn from(offer: OfferIn) -> Self {
        match offer {
            OfferIn::Label(label) => Self {
                label,
                description: None,
            },
            OfferIn::Full { label, description } => Self { label, description },
        }
    }
}

/// One question's answer: the options picked, by index, and the user's own words if any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub picked: Vec<usize>,
    pub text: Option<String>,
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
    /// A question card's answers, one per question, in order.
    Answers(Vec<Answer>),
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
