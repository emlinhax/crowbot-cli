//! Which conversation the session holds: switching to a new or an earlier one, showing one read
//! back, and taking back its last prompt.

use std::path::PathBuf;

use super::Tui;
use crate::agent::setup::going_on;
use crate::conversation::message::Message;
use crate::frontend::tui::feed::Feed;
use crate::frontend::tui::ui;
use crate::frontend::tui::view::View;
use crate::frontend::words;
use crate::session::transcript::{Resumed, Transcript};
use crate::session::undo;
use crate::text::template;
use crate::text::units;

/// Which conversation a switch goes to.
pub(super) enum Switch {
    /// A fresh one, with a new session file.
    New,
    /// The session in this file, from where it stopped.
    Resume(PathBuf),
}

impl Tui<'_> {
    /// A conversation read back from its file: shown as it was, with where it stood.
    pub(super) fn resumed(&mut self, messages: &[Message], resumed: &Resumed, gone: Option<&str>) {
        self.feed.replay(messages);
        self.context_tokens = last_input(messages);
        let cost = units::usd_micros(resumed.spent_micros);
        let text = template::fill(
            &ui::get().text.resumed,
            &[("path", &self.session_file), ("cost", &cost)],
        );
        self.feed.notice(&text, "muted");
        if let Some(gone) = gone {
            let text = words::model_gone(gone, &self.next.model.id);
            self.feed.notice(&text, "warn");
        }
    }

    /// Puts the conversation away and starts the one `to` names: a clean screen and nothing the
    /// model was told, then a new session file or an earlier one read back. A file that cannot
    /// be read leaves `old` as it was.
    pub(super) fn switch_to(&mut self, old: Transcript, to: Switch) -> Transcript {
        let (transcript, resumed) = match to {
            Switch::New => (Transcript::create(&self.app.paths), None),
            Switch::Resume(path) => match Transcript::open(&path) {
                Ok((transcript, resumed)) => (transcript, Some(resumed)),
                Err(e) => {
                    self.feed.notice(&format!("{e:#}"), "error");
                    return old;
                }
            },
        };
        let put_away = std::mem::replace(
            &mut self.session_file,
            transcript.file().display().to_string(),
        );
        // Going on with a session put away earlier: it is saved once, as the last one.
        self.earlier.retain(|path| *path != self.session_file);
        self.shared.start_over();
        let mut gone = None;
        if let Some(resumed) = &resumed {
            match going_on(self.app, &self.catalog, resumed.state.as_ref(), true) {
                Ok(going) => {
                    if going.model.id != self.next.model.id {
                        self.next.switch(&self.app.paths, going.model);
                    }
                    self.next.asked = going.effort;
                    self.shared.set_mode(going.mode);
                    gone = going.gone;
                }
                Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
            }
        }
        // AGENTS.md may have changed (/init writes it), and the prompt is read at a start.
        self.next.reload(&self.app.paths);
        self.feed = Feed::new();
        self.feed.set_welcome(self.welcome());
        self.view = View::default();
        self.queue.clear();
        self.context_tokens = None;
        let saved = old.path().is_some();
        if saved {
            self.earlier.push(put_away.clone());
        }
        let text = &ui::get().text;
        match &resumed {
            Some(resumed) => {
                self.resumed(&transcript.messages, resumed, gone.as_deref());
                if saved {
                    let said = template::fill(&ui::get().resume.saved, &[("path", &put_away)]);
                    self.feed.notice(&said, "muted");
                }
            }
            None if saved => self.feed.notice(
                &template::fill(&text.new_saved, &[("path", &put_away)]),
                "muted",
            ),
            None => self.feed.notice(&text.new_conversation, "muted"),
        }
        transcript
    }

    /// Takes back the last prompt and what followed it, and puts the prompt in the editor.
    pub(super) fn take_back(&mut self, transcript: &mut Transcript) {
        let words = &ui::get().undo;
        let Some(undone) = undo::undo(transcript) else {
            self.feed.notice(&words.nothing, "muted");
            return;
        };
        // What the model read or was told belongs to the turns taken back.
        self.shared.start_over();
        self.feed = Feed::new();
        self.feed.set_welcome(self.welcome());
        self.feed.replay(&transcript.messages);
        self.view = View::default();
        self.context_tokens = last_input(&transcript.messages);
        let draft = self.editor.text();
        let text = match draft.trim().is_empty() {
            true => undone.prompt,
            false => format!("{}\n{draft}", undone.prompt),
        };
        self.editor.set_text(&text);
        self.feed
            .notice(&format!("{} {}", words.done, words.files_kept), "done");
        if let Some(unsaved) = undone.unsaved {
            let text = template::fill(
                &words::get().unsaved,
                &[
                    ("path", &unsaved.path.display().to_string()),
                    ("error", &unsaved.error),
                ],
            );
            self.feed.notice(&text, "warn");
        }
    }
}

/// Input tokens of the last request in `messages`, for the bar's share of the window.
fn last_input(messages: &[Message]) -> Option<u64> {
    messages.iter().rev().find_map(|m| match m {
        Message::Assistant(reply) => reply.usage.map(|u| u.input),
        _ => None,
    })
}
