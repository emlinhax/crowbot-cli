use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::io;
use crate::session::entry::State;
use crate::session::reader;
use crate::session::store::Store;

/// The conversation in memory, mirrored to its session file.
pub struct Transcript {
    pub messages: Vec<Message>,
    store: Store,
    /// A write failed; the file stopped being written and no longer holds everything.
    unsaved: bool,
}

/// What a reopened session brings beyond its messages.
pub struct Resumed {
    /// What its last prompt went out with, or its last reply's model and effort in a file too old
    /// to say.
    pub state: Option<State>,
    /// What its replies cost, in micro-dollars.
    pub spent_micros: u64,
}

/// Why the session file stopped being written.
#[derive(Debug)]
pub struct Unsaved {
    pub path: PathBuf,
    pub error: String,
}

impl Transcript {
    /// A new conversation, with a session file that is written from its first message.
    pub fn create(paths: &Paths) -> Self {
        Self {
            messages: Vec::new(),
            store: Store::create(paths),
            unsaved: false,
        }
    }

    /// The session in the file at `path`, to go on from the end of its current branch.
    pub fn open(path: &Path) -> anyhow::Result<(Self, Resumed)> {
        let text = io::fs::read_string(path)
            .with_context(|| format!("reading {}", path.display()))?
            .with_context(|| format!("{} is gone", path.display()))?;
        let loaded = reader::read(&text);
        let head = loaded.branch.last().map(|(id, _)| id.clone());
        let messages: Vec<Message> = loaded.branch.into_iter().map(|(_, m)| m).collect();
        let replies = || {
            messages.iter().filter_map(|m| match m {
                Message::Assistant(a) => Some(a),
                _ => None,
            })
        };
        let state = loaded.state.or_else(|| {
            replies().next_back().map(|a| State {
                model: a.model.clone(),
                effort: a.effort.clone(),
                mode: None,
            })
        });
        let spent_micros = replies()
            .filter_map(|a| a.usage)
            .map(|u| u.cost_micros)
            .sum();
        let transcript = Self {
            messages,
            store: Store::open(path.to_path_buf(), head),
            unsaved: false,
        };
        Ok((
            transcript,
            Resumed {
                state,
                spent_micros,
            },
        ))
    }

    /// The session file, once it exists and while it holds the whole conversation.
    pub fn path(&self) -> Option<&Path> {
        (self.store.written() && !self.unsaved).then(|| self.store.path())
    }

    /// Where the session file goes, written or not.
    pub fn file(&self) -> &Path {
        self.store.path()
    }

    /// Names this session's files (plans).
    pub fn id(&self) -> &str {
        self.store.name()
    }

    /// Keeps `message`, then writes it. The first failed write is returned and stops saving; later
    /// messages stay in memory.
    pub fn push(&mut self, message: Message) -> Result<(), Unsaved> {
        self.keep(message, None)
    }

    /// `push` for a prompt, with what it is sent under.
    pub fn push_prompt(&mut self, message: Message, state: State) -> Result<(), Unsaved> {
        self.keep(message, Some(state))
    }

    fn keep(&mut self, message: Message, state: Option<State>) -> Result<(), Unsaved> {
        let written = if self.unsaved {
            Ok(())
        } else {
            self.store.append(&message, state).map_err(|e| Unsaved {
                path: self.store.path().to_path_buf(),
                error: format!("{e:#}"),
            })
        };
        self.unsaved |= written.is_err();
        self.messages.push(message);
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io;

    #[test]
    fn there_is_no_file_to_name_until_something_is_said() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::create(&paths);
        assert!(transcript.path().is_none());
        assert!(transcript.push(Message::user_text("one")).is_ok());
        assert!(
            transcript
                .path()
                .is_some_and(|p| p.starts_with(paths.sessions_dir()))
        );
    }

    #[test]
    fn a_failed_write_keeps_the_message_and_is_reported_once() {
        let root = tempfile::tempdir().unwrap();
        // A file where the home directory should be: every append fails.
        let blocker = root.path().join("blocker");
        io::fs::write_atomic(&blocker, b"", io::fs::Access::Shared).unwrap();
        let paths = Paths::at(blocker.join("home"), root.path().into());
        let mut transcript = Transcript::create(&paths);

        let first = transcript.push(Message::user_text("one"));
        assert!(first.is_err_and(|u| u.path.starts_with(&blocker)));
        assert!(transcript.push(Message::user_text("two")).is_ok());
        assert_eq!(transcript.messages.len(), 2);
        assert!(transcript.path().is_none());
    }

    #[test]
    fn a_reopened_session_goes_on_from_its_last_message_and_says_what_it_ran_under() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("20260930T120000_a1b2c3d4.jsonl");
        let v1 = include_str!("../../tests/fixtures/session/v1.jsonl");
        io::fs::write_atomic(&path, v1.as_bytes(), io::fs::Access::Private).unwrap();

        let (mut transcript, resumed) = Transcript::open(&path).unwrap();
        assert_eq!(transcript.messages.len(), 10);
        assert_eq!(transcript.id(), "20260930T120000_a1b2c3d4");
        // v1 recorded no state, so the last reply's model stands in for it.
        assert_eq!(resumed.state.as_ref().map(|s| s.model.as_str()), Some("m"));
        assert_eq!(resumed.spent_micros, 4840);

        let state = State {
            model: "crow-2".into(),
            effort: Some("high".into()),
            mode: Some("auto".into()),
        };
        transcript
            .push_prompt(Message::user_text("again"), state.clone())
            .unwrap();
        let text = io::fs::read_string(&path).unwrap().unwrap();
        let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["parent"], "m6");
        let (reopened, resumed) = Transcript::open(&path).unwrap();
        assert_eq!(reopened.messages.len(), 11);
        assert_eq!(resumed.state, Some(state));
    }
}
