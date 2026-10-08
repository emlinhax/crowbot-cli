use std::path::{Path, PathBuf};

use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::session::store::Store;

/// The conversation in memory, mirrored to its session file.
pub struct Transcript {
    pub messages: Vec<Message>,
    store: Store,
    /// A write failed; the file stopped being written and no longer holds everything.
    unsaved: bool,
}

/// Why the session file stopped being written.
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

    /// The session file, once it exists and while it holds the whole conversation.
    pub fn path(&self) -> Option<&Path> {
        (self.store.written() && !self.unsaved).then(|| self.store.path())
    }

    /// Where the session file goes, written or not.
    pub fn file(&self) -> &Path {
        self.store.path()
    }

    /// Identifies the session in file names (plans).
    pub fn id(&self) -> &str {
        self.store.name()
    }

    /// Keeps `message`, then writes it. The first failed write is returned and ends saving:
    /// later messages stay in memory only, so a full disk costs the file, not the conversation.
    pub fn push(&mut self, message: Message) -> Result<(), Unsaved> {
        let written = if self.unsaved {
            Ok(())
        } else {
            self.store.append(&message).map_err(|e| Unsaved {
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
}
