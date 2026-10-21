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
use crate::session::entry::Snap;
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
        self.feed.replay(messages, &child_messages);
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

    /// Takes back the last prompt and what followed it, files included. The prompt goes back
    /// in the editor, or, sent `again`, is returned to send; the file keeps both tries.
    pub(super) async fn take_back(
        &mut self,
        transcript: &mut Transcript,
        again: bool,
    ) -> Option<String> {
        let words = &ui::get().undo;
        let Some(undone) = undo::undo(transcript) else {
            self.feed.notice(&words.nothing, "muted");
            return None;
        };
        // What the model read or was told belongs to the turns taken back.
        self.shared.start_over();
        self.feed = Feed::new();
        self.feed.set_welcome(self.welcome());
        self.feed.replay(&transcript.messages, &child_messages);
        self.view = View::default();
        self.context_tokens = last_input(&transcript.messages);
        if let Some(snapshot) = &undone.snapshot {
            self.restore(snapshot).await;
        }
        if let Some(unsaved) = &undone.unsaved {
            let text = template::fill(
                &words::get().unsaved,
                &[
                    ("path", &unsaved.path.display().to_string()),
                    ("error", &unsaved.error),
                ],
            );
            self.feed.notice(&text, "warn");
        }
        if again {
            self.feed.user(&undone.prompt);
            return Some(undone.prompt);
        }
        let draft = self.editor.text();
        let text = match draft.trim().is_empty() {
            true => undone.prompt,
            false => format!("{}\n{draft}", undone.prompt),
        };
        self.editor.set_text(&text);
        self.feed.notice(&words.done, "done");
        None
    }
}

impl Tui<'_> {
    /// Puts back the files the taken-back prompt changed, and says how that went.
    async fn restore(&mut self, snapshot: &Snap) {
        let words = &ui::get().undo;
        let restored = self
            .shadow
            .restore(&snapshot.before, snapshot.after.as_deref())
            .await;
        match restored {
            Ok(restored) => {
                let text = words.restored.of(restored.files);
                self.feed.notice(&text, "done");
                if !restored.kept.is_empty() {
                    let paths = restored.kept.join(", ");
                    let text = template::fill(&words.kept, &[("paths", &paths)]);
                    self.feed.notice(&text, "warn");
                }
                for failed in restored.failed {
                    let text = template::fill(&words.failed, &[("what", &failed)]);
                    self.feed.notice(&text, "warn");
                }
            }
            Err(why) => {
                let text = template::fill(&words.not_restored, &[("why", &why)]);
                self.feed.notice(&text, "warn");
            }
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

/// A subagent's messages, from the session file its `task` result linked, so replay can show its
/// calls under the task; empty when the file is gone or unreadable, leaving that task flat.
fn child_messages(path: &str) -> Vec<Message> {
    Transcript::open(std::path::Path::new(path))
        .map(|(transcript, _)| transcript.messages)
        .unwrap_or_default()
}
