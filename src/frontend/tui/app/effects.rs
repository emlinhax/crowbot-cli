//! What a finished command asks the session to do, and the changes it makes for one.

use std::path::Path;

use super::{Step, Switch, Tui};
use crate::agent::roles::{self, RoleId};
use crate::api::error::said;
use crate::commands::{Effect, Outcome as CommandOutcome};
use crate::config::subagents::Role;
use crate::config::{prefs, settings};
use crate::frontend::tui::account::Account;
use crate::frontend::tui::command::{self, Work};
use crate::frontend::tui::forums::Forums;
use crate::frontend::tui::login::Login;
use crate::frontend::tui::picker::Picker;
use crate::frontend::tui::roles::{Now, Roles};
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
            Work::Catalog(catalog, role) => {
                self.next.catalog = catalog;
                self.next.reload(&self.app.paths);
                self.open_roles(role);
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
                Effect::PickModel {
                    refresh: true,
                    role,
                } => {
                    return Step::Work(command::refresh_catalog(self.app, role));
                }
                Effect::PickModel {
                    refresh: false,
                    role,
                } => self.open_roles(role),
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

    /// Asks for `effort` from the next turn on, and remembers it for the next session.
    pub(super) fn set_effort(&mut self, effort: String) {
        let text = &ui::get().text;
        let mut note = template::fill(&text.effort_set, &[("effort", &effort)]);
        if self.next.model.effort(Some(&effort)).is_none() {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &self.next.model.id)],
            ));
        }
        prefs::remember_effort(&self.app.paths, &effort);
        self.next.asked = Some(effort);
        self.feed.notice(&note, "done");
    }

    /// The `/models` card, at `role`'s model list if given.
    fn open_roles(&mut self, role: Option<RoleId>) {
        let picked = prefs::load(&self.app.paths).subagents;
        let now = Now {
            catalog: &self.next.catalog,
            chat: &self.next.model.id,
            chat_effort: self.next.effort(),
            subagents: &self.next.subagents,
            picked: &picked,
            kept_in: self.app.paths.prefs().display().to_string(),
        };
        self.panel = Some(Box::new(Roles::open(&now, role)));
    }

    /// Subagents of `kind` (every kind when `None`) run on `role` from the next task on, and the
    /// next session too. What they run on is read back through the settings layers, so an
    /// inherited choice shows the config files' again.
    pub(super) fn assign(&mut self, kind: Option<&str>, role: Role) {
        prefs::remember_role(&self.app.paths, kind, role);
        match settings::load(&self.app.paths, &self.app.flags) {
            Ok(settings) => self.next.subagents = settings.subagents,
            Err(e) => {
                self.feed.notice(&format!("{e:#}"), "error");
                return;
            }
        }
        self.next.reload(&self.app.paths);
        let words = &ui::get().roles;
        let chat = &self.next.model.id;
        let (model, _) = roles::chosen(kind, None, &self.next.subagents, chat);
        let effort = roles::chosen_effort(kind, &self.next.subagents, self.next.effort());
        let role_id = kind.unwrap_or(&words.every);
        let text = template::fill(
            &words.assigned,
            &[
                ("role", role_id),
                ("model", model),
                ("effort", effort.unwrap_or(&words.default_effort)),
            ],
        );
        self.feed.notice(&text, "done");
    }

    /// Uses `id` from the next turn on, and remembers it for the next session.
    pub(super) fn switch_model(&mut self, id: &str) {
        let Some(model) = self.next.catalog.get(id).cloned() else {
            return;
        };
        prefs::remember_model(&self.app.paths, id);
        self.next.switch(&self.app.paths, model);
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }
}
