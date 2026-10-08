//! The cards that take the editor's place while crowbot waits on the user: a numbered choice
//! (permission, a finished plan) or the model's questions. One shape, so the session treats
//! every card alike.

use crate::frontend::tui::choice::Choice;
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::questions::Questions;
use crate::io::term::KeyEvent;
use crate::live::prompt::{Prompt, Reply};
use crate::text::styled::Line;

/// What a key did to a card.
pub enum Step {
    Stay,
    Answer(Reply),
}

pub trait Card {
    /// The prompt it answers.
    fn id(&self) -> u64;
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Step;
    /// Typed or pasted text, for a card with a line open for it.
    fn insert(&mut self, text: &str);
    fn render(&self, width: usize) -> Vec<Line>;
}

/// The card for a prompt from the agent.
pub fn from_prompt(id: u64, prompt: &Prompt, width: usize) -> Box<dyn Card> {
    match prompt {
        Prompt::Permission {
            tool,
            asks,
            preview,
            ..
        } => Box::new(Choice::permission(
            id,
            tool,
            asks,
            preview.as_deref(),
            width,
        )),
        Prompt::PlanExit { plan, choices } => Box::new(Choice::plan(id, plan, choices, width)),
        Prompt::Question { questions } => Box::new(Questions::new(id, questions.clone())),
    }
}
