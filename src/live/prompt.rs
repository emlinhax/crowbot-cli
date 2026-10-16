//! Everything that waits on the user has one shape: a `Prompt` goes out as an event, and a `Reply`
//! comes back through `Prompts::answer`. Headless runs answer `Unavailable` to all of them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::live::event::AgentEvent;
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
        /// The user confirms this call every time; no mode answers it for them.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        confirm: bool,
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

/// The prompts waiting on the user, each marked whether it asks permission.
pub struct Prompts {
    waiting: Mutex<HashMap<u64, (bool, oneshot::Sender<Reply>)>>,
    next: AtomicU64,
}

impl Default for Prompts {
    fn default() -> Self {
        Self {
            waiting: Mutex::default(),
            next: AtomicU64::new(1),
        }
    }
}

impl Prompts {
    fn register(&self, permission: bool) -> (u64, oneshot::Receiver<Reply>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, (permission, tx));
        (id, rx)
    }

    /// Drops a prompt nobody will answer now (the run was interrupted).
    fn forget(&self, id: u64) {
        self.waiting.lock().unwrap().remove(&id);
    }

    /// Whether prompt `id` is still waiting for an answer.
    pub fn waiting(&self, id: u64) -> bool {
        self.waiting.lock().unwrap().contains_key(&id)
    }

    /// `false` when that prompt is no longer waiting.
    pub fn answer(&self, id: u64, reply: Reply) -> bool {
        match self.waiting.lock().unwrap().remove(&id) {
            Some((_, waiting)) => waiting.send(reply).is_ok(),
            None => false,
        }
    }

    /// Says yes to every waiting permission prompt; questions still wait for their answer.
    pub fn approve_permissions(&self) {
        let mut waiting = self.waiting.lock().unwrap();
        let permissions: Vec<u64> = waiting
            .iter()
            .filter(|(_, (permission, _))| *permission)
            .map(|(id, _)| *id)
            .collect();
        for id in permissions {
            if let Some((_, prompt)) = waiting.remove(&id) {
                let _ = prompt.send(Reply::Yes);
            }
        }
    }
}

/// Asks and waits; `None` when the run was interrupted first.
pub async fn ask(
    prompts: &Prompts,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
    call_id: &str,
    prompt: Prompt,
    cancel: &CancellationToken,
) -> Option<Reply> {
    // A confirmation is the user's every time, so a switch to a mode that asks nothing leaves it.
    let approvable = matches!(prompt, Prompt::Permission { confirm: false, .. });
    let (id, reply) = prompts.register(approvable);
    emit(AgentEvent::Prompt {
        id,
        call_id: call_id.to_owned(),
        prompt,
    });
    tokio::select! {
        reply = reply => Some(reply.unwrap_or(Reply::Unavailable)),
        () = cancel.cancelled() => {
            prompts.forget(id);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_mode_that_asks_nothing_approves_permissions_but_not_confirmations() {
        let prompts = Prompts::default();
        let cancel = CancellationToken::new();
        let emit = |_: AgentEvent| {};
        let permission = |confirm| Prompt::Permission {
            tool: "forum_post".into(),
            asks: Vec::new(),
            preview: None,
            confirm,
        };
        let plain = ask(&prompts, &emit, "a", permission(false), &cancel);
        let confirm = ask(&prompts, &emit, "b", permission(true), &cancel);
        tokio::pin!(plain, confirm);
        assert!(futures_util::poll!(&mut plain).is_pending());
        assert!(futures_util::poll!(&mut confirm).is_pending());
        prompts.approve_permissions();
        assert!(matches!(plain.await, Some(Reply::Yes)));
        assert!(prompts.waiting(2), "the confirmation still waits");
    }
}
