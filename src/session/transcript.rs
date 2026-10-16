use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::io;
use crate::session::entry::{self, Snap, State};
use crate::session::reader;
use crate::session::store::Store;

/// The conversation in memory, mirrored to its session file.
pub struct Transcript {
    pub messages: Vec<Message>,
    /// Each message's entry id in the file, in step with `messages`.
    ids: Vec<String>,
    /// Each prompt's snapshots, by its entry id.
    snapshots: HashMap<String, Snap>,
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
            ids: Vec::new(),
            snapshots: HashMap::new(),
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
        let (ids, messages): (Vec<String>, Vec<Message>) = loaded.branch.into_iter().unzip();
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
            ids,
            snapshots: loaded.snapshots,
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
        let id = entry::new_id();
        let written = self.saved(|store| store.append(&id, &message, state));
        self.messages.push(message);
        self.ids.push(id);
        written
    }

    /// The entry id of the last message, the run's prompt right after it is said.
    pub fn last_id(&self) -> Option<&str> {
        self.ids.last().map(String::as_str)
    }

    /// Records prompt `of`'s snapshots, in memory and in the file.
    pub fn snapshot(&mut self, of: &str, trees: Snap) -> Result<(), Unsaved> {
        let written = self.saved(|store| store.snapshot(of, &trees));
        self.snapshots.insert(of.to_owned(), trees);
        written
    }

    /// The snapshots of the message at `index`, when it is a prompt that changed files.
    pub fn snapshot_at(&self, index: usize) -> Option<&Snap> {
        self.snapshots.get(self.ids.get(index)?)
    }

    /// Takes back every message from `len` on; the file keeps them off its current branch.
    pub fn rewind(&mut self, len: usize) -> Result<(), Unsaved> {
        self.messages.truncate(len);
        self.ids.truncate(len);
        let to = self.ids.last().cloned();
        self.saved(|store| store.rewind(to.as_deref()))
    }

    /// Runs `write` while the file is still written; the first failure stops saving.
    fn saved(
        &mut self,
        write: impl FnOnce(&mut Store) -> anyhow::Result<()>,
    ) -> Result<(), Unsaved> {
        if self.unsaved {
            return Ok(());
        }
        let written = write(&mut self.store).map_err(|e| Unsaved {
            path: self.store.path().to_path_buf(),
            error: format!("{e:#}"),
        });
        self.unsaved |= written.is_err();
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

    #[test]
    fn a_rewind_takes_messages_back_and_the_next_one_follows_where_it_went() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::create(&paths);
        for text in ["one", "reply", "two", "reply two"] {
            transcript.push(Message::user_text(text)).unwrap();
        }
        transcript.rewind(2).unwrap();
        assert_eq!(transcript.messages.len(), 2);
        let path = transcript.path().unwrap().to_path_buf();
        let (reopened, _) = Transcript::open(&path).unwrap();
        assert_eq!(reopened.messages, transcript.messages);

        transcript.push(Message::user_text("two again")).unwrap();
        let (reopened, _) = Transcript::open(&path).unwrap();
        assert_eq!(reopened.messages.len(), 3);
        assert_eq!(reopened.messages[2], Message::user_text("two again"));
        // The taken-back pair stays in the file.
        let text = io::fs::read_string(&path).unwrap().unwrap();
        assert!(text.contains("reply two"));
    }
}
