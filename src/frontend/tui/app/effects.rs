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
use crate::frontend::tui::picker::{Picker, Show};
use crate::frontend::tui::signup::Signup;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::ui;
use crate::io::{clipboard, clock};
use crate::mode;
use crate::models::{choice, preset};
use crate::session;
use crate::text::template;

impl<'a> Tui<'a> {
    /// Applies what a command finished with.
    pub(super) fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.open_models(Show::All);
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
                Effect::PickModel { refresh: false } => self.open_models(Show::All),
                Effect::PickPreset => self.open_models(Show::Presets),
                Effect::NewPreset { name } => self.new_preset(name.as_deref()),
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

    /// The model picker, on what `show` lists; presets are read again, so one just written shows.
    fn open_models(&mut self, show: Show) {
        let paths = &self.app.paths;
        let listed = preset::load(paths);
        let current = self.next.choice.id();
        let picker = Picker::models(&self.catalog, &listed, &paths.presets(), current, show);
        self.panel = Some(Box::new(picker));
    }

    /// Writes a new preset on the session's model, named `name` or the first free `new`, and
    /// says where to edit it.
    pub(super) fn new_preset(&mut self, name: Option<&str>) {
        let model = &self.next.choice.model().id;
        match preset::create(&self.app.paths, name, model) {
            Ok(path) => {
                let path = path.display().to_string();
                let text = template::fill(&ui::get().picker.preset_written, &[("path", &path)]);
                self.feed.notice(&text, "done");
            }
            Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
        }
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
        let model = self.next.choice.model();
        if model.effort(Some(&effort)).is_none() {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &model.id)],
            ));
        }
        prefs::remember_effort(&self.app.paths, &effort);
        self.next.asked = Some(effort);
        self.feed.notice(&note, "done");
    }

    /// Uses `id` from the next turn on, and remembers it for the next session.
    pub(super) fn switch_model(&mut self, id: &str) {
        let presets = preset::load(&self.app.paths);
        let choice = match choice::find(&self.catalog, &presets, id) {
            Ok(choice) => choice,
            Err(e) => return self.feed.notice(&format!("{e:#}"), "error"),
        };
        prefs::remember_model(&self.app.paths, id);
        self.next.switch(&self.app.paths, choice);
        self.served = None;
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }
}
