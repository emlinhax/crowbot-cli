use std::path::{Path, PathBuf};

use crate::agent::message::Message;
use crate::session::store::Store;

/// The conversation in memory, mirrored to its session file when it has one.
pub struct Transcript {
    pub messages: Vec<Message>,
    store: Option<Store>,
    /// A write failed; the file stopped being written and no longer holds everything.
    unsaved: bool,
}

/// Why the session file stopped being written.
pub struct Unsaved {
    pub path: PathBuf,
    pub error: String,
}

impl Transcript {
    pub fn new(store: Option<Store>) -> Self {
        Self {
            messages: Vec::new(),
            store,
            unsaved: false,
        }
    }

    /// The session file, once it exists and while it holds the whole conversation.
    pub fn path(&self) -> Option<&Path> {
        self.store
            .as_ref()
            .filter(|s| s.written() && !self.unsaved)
            .map(|s| s.path.as_path())
    }

    /// Identifies the session in file names (plans); stable for a stored session.
    pub fn id(&self) -> String {
        self.store
            .as_ref()
            .map_or_else(|| "unsaved".to_owned(), Store::stem)
    }

    /// Keeps `message`, then writes it. The first failed write is returned and ends saving:
    /// later messages stay in memory only, so a full disk costs the file, not the conversation.
    pub fn push(&mut self, message: Message) -> Result<(), Unsaved> {
        let written = match &mut self.store {
            Some(store) if !self.unsaved => store.append(&message).map_err(|e| Unsaved {
                path: store.path.clone(),
                error: format!("{e:#}"),
            }),
            _ => Ok(()),
        };
        self.unsaved |= written.is_err();
        self.messages.push(message);
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::Part;
    use crate::io;
    use crate::paths::Paths;

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    #[test]
    fn there_is_no_file_to_name_until_something_is_said() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::new(Some(Store::create(&paths)));
        assert!(transcript.path().is_none());
        assert!(transcript.push(user("one")).is_ok());
        assert!(
            transcript
                .path()
                .is_some_and(|p| p.starts_with(paths.sessions_dir()))
        );
    }

    #[test]
    fn a_failed_write_keeps_the_message_and_is_reported_once() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut store = Store::create(&paths);
        // A file where its directory should be: every later append fails.
        let blocker = root.path().join("blocker");
        io::fs::write_atomic(&blocker, b"", io::fs::Access::Shared).unwrap();
        store.path = blocker.join("session.jsonl");
        let mut transcript = Transcript::new(Some(store));

        let first = transcript.push(user("one"));
        assert!(first.is_err_and(|u| u.path.ends_with("session.jsonl")));
        assert!(transcript.push(user("two")).is_ok());
        assert_eq!(transcript.messages.len(), 2);
        assert!(transcript.path().is_none());
    }
}
