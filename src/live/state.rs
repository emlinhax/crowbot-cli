//! Session state shared by a run and the frontend driving it, which may change it mid-run.

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::conversation::message::Part;
use crate::live::files::Files;
use crate::live::prompt::Prompts;
use crate::mode::Mode;

/// A mode change the model has to hear about.
pub enum Reminder {
    /// A mode with instructions of its own.
    Entered(&'static Mode),
    /// Out of a mode with instructions, into one without, so they no longer hold.
    Left { now: &'static Mode },
}

pub struct Shared {
    cancel: Mutex<CancellationToken>,
    steer: Mutex<VecDeque<Vec<Part>>>,
    follow: Mutex<VecDeque<Vec<Part>>>,
    pub prompts: Prompts,
    mode: Mutex<&'static Mode>,
    /// The mode the model was last told about.
    reminded: Mutex<Option<&'static Mode>>,
    pub files: Files,
}

impl Shared {
    pub fn new(mode: &'static Mode) -> Self {
        Self {
            cancel: Mutex::new(CancellationToken::new()),
            steer: Mutex::default(),
            follow: Mutex::default(),
            prompts: Prompts::default(),
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

    /// Switching into a mode that allows everything also approves permission prompts already
    /// waiting (questions still wait for their answer).
    pub fn set_mode(&self, mode: &'static Mode) {
        *self.mode.lock().unwrap() = mode;
        if mode.asks_nothing() {
            self.prompts.approve_permissions();
        }
    }

    /// What to tell the model about the mode now, if it changed since the model was last told.
    pub fn take_reminder(&self) -> Option<Reminder> {
        let mode = self.mode();
        let mut reminded = self.reminded.lock().unwrap();
        if reminded.is_some_and(|m| m.id == mode.id) {
            return None;
        }
        let left = reminded.replace(mode);
        if mode.reminder.is_some() {
            Some(Reminder::Entered(mode))
        } else if left.is_some_and(|m| m.reminder.is_some()) {
            Some(Reminder::Left { now: mode })
        } else {
            None
        }
    }

    /// What the model was told about the mode was summarised away; the next prompt tells it again.
    pub fn forget_reminded(&self) {
        *self.reminded.lock().unwrap() = None;
    }

    /// A new conversation: the model has been told nothing, read nothing, and nothing waits.
    pub fn start_over(&self) {
        self.forget_reminded();
        self.steer.lock().unwrap().clear();
        self.follow.lock().unwrap().clear();
        self.files.forget_all();
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
}
