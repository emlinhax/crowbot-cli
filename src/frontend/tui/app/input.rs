//! What a key, a click or a paste does in the session: cards first, then the command popup,
//! then the editor and its actions.

use super::{Step, Switch, Tui};
use crate::commands::{self, Scope};
use crate::config::limits;
use crate::conversation::message::Part;
use crate::frontend::tui::card::{self, Next};
use crate::frontend::tui::keymap::{self, Action};
use crate::frontend::tui::queue::Kind;
use crate::frontend::tui::{command, forums, palette};
use crate::io::clock;
use crate::io::term::{Button, Input, KeyCode, KeyEvent};
use crate::mode;
use crate::text::styled::TAB;

impl<'a> Tui<'a> {
    /// `running` is a turn in flight, `busy` a job the session card waits on.
    pub(super) fn input(&mut self, input: Input, running: bool, busy: bool) -> Step<'a> {
        // Inside a burst of keys (a paste where the terminal sends one as keystrokes), Enter
        // and Tab are text for whatever field has the keys, never commands.
        let input = match input {
            Input::Key(key) => match (self.burst.is_paste(clock::instant()), key.code) {
                (true, KeyCode::Enter) => Input::Paste("\n".into()),
                (true, KeyCode::Tab) => Input::Paste(TAB.into()),
                _ => Input::Key(key),
            },
            other => other,
        };
        match input {
            Input::Resize(width, height) => {
                self.screen.resize(width, height);
                Step::Continue
            }
            Input::Scroll(notches) => {
                let lines = limits::get().tui.scroll_lines.value as isize;
                self.view.scroll(notches * lines);
                Step::Continue
            }
            Input::Click {
                row,
                button: Button::Left,
            } => {
                if let Some(Some(block)) = self.blocks.get(row) {
                    self.feed.toggle(*block);
                }
                Step::Continue
            }
            // What the session card offers (the login code) wherever it is clicked, else the
            // block under the pointer.
            Input::Click {
                row,
                button: Button::Right,
            } => {
                let text = self
                    .panel
                    .as_ref()
                    .and_then(|card| card.copied())
                    .map(str::to_owned)
                    .or_else(|| {
                        let block = self.blocks.get(row).copied().flatten()?;
                        self.feed.copy_text(block)
                    });
                if let Some(text) = text {
                    self.copy(&text);
                }
                Step::Continue
            }
            Input::Paste(text) => {
                if let Some(card) = &mut self.panel {
                    card.insert(&text);
                } else if let Some((_, card)) = self.cards.front_mut() {
                    card.insert(&text);
                } else {
                    self.editor.insert(&text);
                }
                Step::Continue
            }
            Input::Key(key) => self.key(&key, running, busy),
        }
    }

    /// A key: paging scrolls wherever the keys go, a card on screen takes the rest, and
    /// otherwise the palette and then the editor do.
    pub(super) fn key(&mut self, key: &KeyEvent, running: bool, busy: bool) -> Step<'a> {
        let action = keymap::get().action(key);
        if let Some(paging @ (Action::PageUp | Action::PageDown)) = action {
            let page = self.view.page();
            let rows = if paging == Action::PageUp {
                -page
            } else {
                page
            };
            self.view.scroll(rows);
            return Step::Continue;
        }
        if self.panel.is_some() || !self.cards.is_empty() {
            return self.card_key(action, key, running, busy);
        }
        match action {
            Some(action) => {
                // The command popup takes its keys first; it is closed during a run,
                // where Tab steers.
                if !running {
                    match self.palette.key(action, &self.editor.text()) {
                        palette::Step::Ignored => {}
                        palette::Step::Handled => return Step::Continue,
                        palette::Step::Complete(text) => {
                            self.editor.set_text(&text);
                            return Step::Continue;
                        }
                        palette::Step::Run(name) => {
                            self.editor.set_text(&Scope::Session.invoke(&name));
                            return self.submit(running);
                        }
                    }
                }
                self.action(action, running)
            }
            None => {
                self.editor.type_key(key);
                Step::Continue
            }
        }
    }

    /// Keys while a card is up: the session keeps a few, the card takes the rest.
    pub(super) fn card_key(
        &mut self,
        action: Option<Action>,
        key: &KeyEvent,
        running: bool,
        busy: bool,
    ) -> Step<'a> {
        match action {
            Some(Action::CycleMode) => {
                self.cycle_mode();
                return Step::Continue;
            }
            Some(Action::CtrlC) if running => {
                self.interrupt();
                return Step::Continue;
            }
            // A session card waiting on its job closes, which drops the job.
            Some(Action::Escape | Action::CtrlC) if busy && self.panel.is_some() => {
                self.panel = None;
                return Step::Continue;
            }
            _ => {}
        }
        if let Some(card) = &mut self.panel {
            let next = card.key(action, key);
            return self.panel_next(next);
        }
        if let Some((id, card)) = self.cards.front_mut()
            && let Next::Answer(reply) = card.key(action, key)
        {
            self.shared.prompts.answer(*id, reply);
            self.cards.pop_front();
        }
        Step::Continue
    }

    /// Does what the session card asked for.
    pub(super) fn panel_next(&mut self, next: Next) -> Step<'a> {
        match next {
            // A session card answers no prompt.
            Next::Stay | Next::Answer(_) => {}
            Next::Run(job) => return Step::Job(card::run(self.app, job)),
            Next::Note(text) => self.feed.notice(&text, "done"),
            Next::Error(text) => self.feed.notice(&text, "error"),
            Next::Close(message) => {
                self.panel = None;
                if let Some(message) = message {
                    self.feed.notice(&message, "done");
                }
            }
            Next::UseModel(id) => {
                self.panel = None;
                self.switch_model(&id);
            }
            Next::Resume(path) => {
                self.panel = None;
                return Step::Session(Switch::Resume(path));
            }
        }
        Step::Continue
    }

    /// Hands a card's finished work back to it; what it asks next may be more work.
    pub(super) fn job_done(&mut self, done: card::Done) -> Step<'a> {
        // The prompt names the forums to browse; an added or removed one changes it.
        if matches!(done, card::Done::Forums(forums::Done::Changed { .. })) {
            self.next.reload(&self.app.paths);
        }
        // What the account card learned is what the bar and welcome show.
        if let card::Done::Account(done) = &done
            && let Some(me) = done.me()
        {
            self.account_seen(me.clone());
        }
        match self.panel.as_mut().map(|card| card.finished(done)) {
            Some(next) => self.panel_next(next),
            None => Step::Continue,
        }
    }

    pub(super) fn action(&mut self, action: Action, running: bool) -> Step<'a> {
        match action {
            Action::Submit => return self.submit(running),
            // A command is never steering: it would reach the model as text.
            Action::Tab if running && self.is_command() => return self.submit(running),
            Action::Tab if running => self.enqueue(Kind::Steering),
            Action::Tab => self.complete(),
            Action::CycleMode => self.cycle_mode(),
            Action::Escape if running => self.interrupt(),
            Action::Escape => {}
            Action::CtrlC => {
                if !self.editor.is_empty() {
                    self.editor.clear();
                } else if running {
                    self.interrupt();
                } else {
                    let now = clock::instant();
                    let window = limits::get().tui.quit_window_ms.ms();
                    if self
                        .last_ctrl_c
                        .is_some_and(|t| now.duration_since(t) <= window)
                    {
                        return Step::Quit;
                    }
                    self.last_ctrl_c = Some(now);
                }
            }
            Action::Quit if self.editor.is_empty() => return Step::Quit,
            Action::Quit => {}
            Action::ToggleReasoning => self.feed.toggle_all(),
            editing => {
                self.editor.apply(editing);
            }
        }
        Step::Continue
    }

    pub(super) fn submit(&mut self, running: bool) -> Step<'a> {
        let text = self.editor.text();
        // A trailing backslash asks for a new line, for terminals that cannot send Shift+Enter.
        if let Some(stripped) = text.strip_suffix('\\') {
            self.editor.set_text(&format!("{stripped}\n"));
            return Step::Continue;
        }
        if text.trim().is_empty() {
            return Step::Continue;
        }
        let prefix = Scope::Session.prefix();
        let escape = prefix.repeat(2);
        // A doubled prefix sends the line as written, less one `/`: `//etc/hosts is odd`.
        if text.trim_start().starts_with(&escape) {
            self.editor.set_text(&text.replacen(&escape, prefix, 1));
        } else if let Some(line) = text.trim().strip_prefix(prefix) {
            // Checked before a run gets the text: a command queued as a message would send
            // its arguments (a `--key`) to the model.
            let line = line.to_owned();
            let shown = Scope::Session.invoke(&commands::redact_line(&line));
            self.editor.take_remembering(shown.clone());
            return self.command(&line, &shown);
        }
        if running {
            self.enqueue(Kind::Queued);
            return Step::Continue;
        }
        let text = self.editor.take();
        self.feed.user(&text);
        Step::Send(text)
    }

    /// Hands typed text to the run: steering lands after the current tools, a queued message
    /// when the run would otherwise stop.
    pub(super) fn enqueue(&mut self, kind: Kind) {
        let text = self.editor.take();
        if text.trim().is_empty() {
            return;
        }
        let parts = vec![Part::Text { text: text.clone() }];
        match kind {
            Kind::Steering => self.shared.steer(parts),
            Kind::Queued => self.shared.follow_up(parts),
        }
        self.queue.push((kind, text));
    }

    /// Stops the run and puts anything still queued back in the editor.
    pub(super) fn interrupt(&mut self) {
        self.shared.interrupt();
        let mut pulled: Vec<String> = self
            .shared
            .drain_steer()
            .into_iter()
            .chain(self.shared.drain_follow())
            .map(|parts| Part::text_of(&parts))
            .collect();
        self.queue.clear();
        if !pulled.is_empty() {
            if !self.editor.is_empty() {
                pulled.push(self.editor.text());
            }
            self.editor.set_text(&pulled.join("\n"));
        }
    }

    pub(super) fn cycle_mode(&mut self) {
        self.shared.set_mode(mode::next(self.shared.mode()));
    }

    /// Completes a slash command name when only one fits.
    pub(super) fn complete(&mut self) {
        let text = self.editor.text();
        if let [only] = palette::matches(&text).as_slice() {
            self.editor.set_text(&palette::completion(only));
        }
    }

    pub(super) fn is_command(&self) -> bool {
        self.editor
            .text()
            .trim_start()
            .starts_with(Scope::Session.prefix())
    }

    /// Runs the command `line`; `shown` is how it is echoed, secrets hidden.
    pub(super) fn command(&mut self, line: &str, shown: &str) -> Step<'a> {
        match command::start(self.app, line, self.session()) {
            Ok(run) => {
                self.feed.user(shown);
                Step::Work(run)
            }
            Err(note) => {
                if !note.is_empty() {
                    self.feed.notice(&note, "warn");
                }
                Step::Continue
            }
        }
    }
}
