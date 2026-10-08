//! What the next turn is sent with: the model, the effort asked for, and the system prompt
//! written for that model.

use crate::agent::system_prompt;
use crate::api::models::Model;
use crate::config::paths::Paths;

#[derive(Clone)]
pub struct NextTurn {
    pub model: Model,
    /// The effort asked for: the setting until `/effort` changes it.
    pub asked: Option<String>,
    pub system: String,
}

impl NextTurn {
    pub fn new(paths: &Paths, model: Model, asked: Option<String>) -> Self {
        let system = system_prompt::build(paths, &model.id);
        Self {
            model,
            asked,
            system,
        }
    }

    /// The effort the model is sent: the one asked for, when the model takes it.
    pub fn effort(&self) -> Option<&str> {
        self.model.effort(self.asked.as_deref())
    }

    /// Uses `model` from the next turn on, with a prompt written for it.
    pub fn switch(&mut self, paths: &Paths, model: Model) {
        self.system = system_prompt::build(paths, &model.id);
        self.model = model;
    }

    /// Writes the prompt again, since what it names (AGENTS.md, the forums) may have changed.
    pub fn reload(&mut self, paths: &Paths) {
        self.system = system_prompt::build(paths, &self.model.id);
    }
}
