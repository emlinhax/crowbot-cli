//! Session state a run shares with whoever drives it: queued messages, pending permission
//! prompts, approvals, the mode, and the interrupt. Frontends change it while a run is live.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::message::Part;
use crate::mode::Mode;
use crate::permission::rule::{Action, Rule};
use crate::tools::files::Files;

/// An answer to a permission prompt.
// CEILING: headless runs only ever answer `Unavailable`; the TUI (M3) answers the rest.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Once,
    /// Allow now, and the same thing for the rest of the session.
    Always,
    /// No; with feedback the model hears why and carries on, without it the run stops.
    Reject {
        feedback: Option<String>,
    },
    /// Nobody can answer (a headless run).
    Unavailable,
}

pub struct Shared {
    cancel: Mutex<CancellationToken>,
    steer: Mutex<VecDeque<Vec<Part>>>,
    follow: Mutex<VecDeque<Vec<Part>>>,
    asks: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next_ask: AtomicU64,
    approved: Mutex<Vec<Rule>>,
    mode: Mutex<&'static Mode>,
    /// The mode whose reminder the model was last given.
    reminded: Mutex<Option<String>>,
    pub files: Files,
}

impl Shared {
    pub fn new(mode: &'static Mode) -> Self {
        Self {
            cancel: Mutex::new(CancellationToken::new()),
            steer: Mutex::default(),
            follow: Mutex::default(),
            asks: Mutex::default(),
            next_ask: AtomicU64::new(1),
            approved: Mutex::default(),
            mode: Mutex::new(mode),
            reminded: Mutex::default(),
            files: Files::default(),
        }
    }

    /// A fresh interrupt for a new run.
    pub fn begin_run(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.cancel.lock().unwrap() = token.clone();
        token
    }

    pub fn interrupt(&self) {
        self.cancel.lock().unwrap().cancel();
    }

    pub fn mode(&self) -> &'static Mode {
        *self.mode.lock().unwrap()
    }

    /// Switching into a mode that allows everything also clears prompts already waiting.
    pub fn set_mode(&self, mode: &'static Mode) {
        *self.mode.lock().unwrap() = mode;
        if mode.verdicts.ask == Action::Allow {
            for (_, waiting) in self.asks.lock().unwrap().drain() {
                let _ = waiting.send(Reply::Once);
            }
        }
    }

    /// The reminder to give the model now, if the mode changed since the last one.
    pub fn take_reminder(&self) -> Option<&'static Mode> {
        let mode = self.mode();
        let mut reminded = self.reminded.lock().unwrap();
        if reminded.as_deref() == Some(mode.id.as_str()) {
            return None;
        }
        *reminded = Some(mode.id.clone());
        mode.reminder.is_some().then_some(mode)
    }

    /// Delivered after the current tool calls finish.
    pub fn steer(&self, parts: Vec<Part>) {
        self.steer.lock().unwrap().push_back(parts);
    }

    /// Delivered when the run would otherwise stop.
    pub fn follow_up(&self, parts: Vec<Part>) {
        self.follow.lock().unwrap().push_back(parts);
    }

    pub fn drain_steer(&self) -> Vec<Vec<Part>> {
        self.steer.lock().unwrap().drain(..).collect()
    }

    pub fn drain_follow(&self) -> Vec<Vec<Part>> {
        self.follow.lock().unwrap().drain(..).collect()
    }

    pub fn register_ask(&self) -> (u64, oneshot::Receiver<Reply>) {
        let id = self.next_ask.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.asks.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    /// `false` when that prompt is no longer waiting.
    pub fn answer(&self, id: u64, reply: Reply) -> bool {
        match self.asks.lock().unwrap().remove(&id) {
            Some(waiting) => waiting.send(reply).is_ok(),
            None => false,
        }
    }

    pub fn approve(&self, rules: impl IntoIterator<Item = Rule>) {
        self.approved.lock().unwrap().extend(rules);
    }

    pub fn approved(&self) -> Vec<Rule> {
        self.approved.lock().unwrap().clone()
    }
}
