//! What a finished command asks the session to do, and the changes it makes for one.

use std::path::Path;

use super::{Step, Switch, Tui};
use crate::api::error::said;
use crate::commands::{Effect, Outcome as CommandOutcome};
use crate::config::prefs;
use crate::frontend::tui::account::Account;
use crate::frontend::tui::command::{self, Work};
use crate::frontend::tui::forums::Forums;
use crate::frontend::tui::login::Login;
use crate::frontend::tui::models::Models;
use crate::frontend::tui::picker::Picker;
use crate::frontend::tui::signup::Signup;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::ui;
use crate::io::{clipboard, clock};
use crate::mode;
use crate::session;
use crate::text::template;

impl<'a> Tui<'a> {
    /// Applies what a command finished with.
    pub(super) fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.open_models();
                return Step::Continue;
            }
            Work::Command(Err(e)) => {
                self.feed.notice(&said(&e), "error");
                return Step::Continue;
            }
            Work::Command(Ok(outcome)) => outcome,
            Work::Account { changes, result } => {
                self.account_landed(changes, result);
                return Step::Continue;
            }
        };
        if !outcome.text.is_empty() {
            self.feed.markdown(&outcome.text);
        }
        for effect in outcome.effects {
            match effect {
                Effect::Quit => return Step::Quit,
                Effect::CycleMode => self.cycle_mode(),
                Effect::SetMode(id) => {
                    if let Some(mode) = mode::get(&id) {
                        self.shared.set_mode(mode);
                    }
                }
                Effect::Login { number } => {
                    let (card, next) = Login::open(&self.app.paths, number, self.draw_width());
                    self.panel = Some(Box::new(card));
                    return self.panel_next(next);
                }
                Effect::Account => {
                    let (card, next) = Account::open(self.draw_width());
                    self.panel = Some(Box::new(card));
                    return self.panel_next(next);
                }
                Effect::Signup { force } => {
                    let (card, next) = Signup::open(&self.app.paths, force, self.draw_width());
                    self.panel = Some(Box::new(card));
                    return self.panel_next(next);
                }
                Effect::PickModel { refresh: true } => {
                    return Step::Work(command::refresh_catalog(self.app));
                }
                Effect::PickModel { refresh: false } => self.open_models(),
                Effect::Forums { login } => match Forums::open(&self.app.paths, login.as_deref()) {
                    Ok(card) => self.panel = Some(Box::new(card)),
                    Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
                },
                Effect::NewConversation => return Step::Session(Switch::New),
                Effect::Undo => return Step::Undo { again: false },
                Effect::Retry => return Step::Undo { again: true },
                Effect::Compact { focus } => return Step::Compact(focus),
                Effect::Resume => {
                    let current = Path::new(&self.session_file);
                    let list: Vec<_> = session::list::list(&self.app.paths)
                        .into_iter()
                        .filter(|s| s.path != current)
                        .collect();
                    match list.is_empty() {
                        true => self.feed.notice(&ui::get().resume.none, "muted"),
                        false => self.panel = Some(Box::new(Picker::sessions(list))),
                    }
                }
                Effect::SetEffort(id) => self.set_effort(id),
                Effect::Send(text) => return Step::Send(text),
                Effect::CopyLastReply => match self.feed.last_reply() {
                    Some(text) => self.copy(&text),
                    None => self.feed.notice(&ui::get().text.nothing_to_copy, "muted"),
                },
            }
        }
        Step::Continue
    }

    /// Puts `text` on the clipboard and says so at the top right.
    pub(super) fn copy(&mut self, text: &str) {
        clipboard::copy(text);
        self.toast = Some(Toast::new(&ui::get().text.copied, clock::instant()));
    }

    /// Opens the `/models` card: the main agent's model and effort, then each role's.
    pub(super) fn open_models(&mut self) {
        let card = Models::new(
            &self.catalog,
            &self.app.paths,
            (&self.next.model, self.next.effort()),
        );
        self.panel = Some(Box::new(card));
    }

    /// Asks for `effort` from the next turn on, and remembers it for the next session.
    pub(super) fn set_effort(&mut self, effort: String) {
        self.ask_effort(Some(effort));
    }

    /// Asks for `effort` (`None`: the model's own), remembering it and writing the prompt again,
    /// since it names each role's effort.
    pub(super) fn ask_effort(&mut self, effort: Option<String>) {
        let text = &ui::get().text;
        let words = &ui::get().models;
        let mut note = match &effort {
            Some(level) => template::fill(&words.effort_set, &[("effort", level)]),
            None => text.effort_default.clone(),
        };
        if effort
            .as_deref()
            .is_some_and(|level| self.next.model.effort(Some(level)).is_none())
        {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &self.next.model.id)],
            ));
        }
        match &effort {
            Some(level) => prefs::remember_effort(&self.app.paths, level),
            None => prefs::forget_effort(&self.app.paths),
        }
        self.next.ask(&self.app.paths, effort);
        self.feed.notice(&note, "done");
    }

    /// Uses `id` from the next turn on, and remembers it for the next session.
    pub(super) fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        prefs::remember_model(&self.app.paths, id);
        self.next.switch(&self.app.paths, model);
        let text = template::fill(&ui::get().models.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }
}
