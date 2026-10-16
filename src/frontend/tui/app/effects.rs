//! What a finished command asks the session to do, and the changes it makes for one.

use super::{Step, Tui};
use crate::api::error::said;
use crate::commands::{Effect, Outcome as CommandOutcome};
use crate::frontend::tui::account::Account;
use crate::frontend::tui::command::{self, Work};
use crate::frontend::tui::forums::Forums;
use crate::frontend::tui::login::Login;
use crate::frontend::tui::picker::Picker;
use crate::frontend::tui::signup::Signup;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::ui;
use crate::io::{clipboard, clock};
use crate::mode;
use crate::text::template;

impl<'a> Tui<'a> {
    /// Applies what a command finished with.
    pub(super) fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.panel = Some(Box::new(Picker::new(&self.catalog, &self.next.model.id)));
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
                Effect::PickModel { refresh: false } => {
                    self.panel = Some(Box::new(Picker::new(&self.catalog, &self.next.model.id)));
                }
                Effect::Forums { login } => match Forums::open(&self.app.paths, login.as_deref()) {
                    Ok(card) => self.panel = Some(Box::new(card)),
                    Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
                },
                Effect::NewConversation => return Step::NewConversation,
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

    /// Asks for `effort` from the next turn on, for this session only.
    pub(super) fn set_effort(&mut self, effort: String) {
        let text = &ui::get().text;
        let mut note = template::fill(&text.effort_set, &[("effort", &effort)]);
        if self.next.model.effort(Some(&effort)).is_none() {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &self.next.model.id)],
            ));
        }
        self.next.asked = Some(effort);
        self.feed.notice(&note, "done");
    }

    /// Uses `id` from the next turn on, for this session only.
    pub(super) fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        self.next.switch(&self.app.paths, model);
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }
}
