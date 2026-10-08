use std::path::{Path, PathBuf};

use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::io;
use crate::release;
use crate::session::entry::{self, Entry, FORMAT_VERSION};

/// Append-only JSONL session file and its only writer. Nothing is written before the first message.
pub struct Store {
    /// `{stamp}_{id}`: the file's name without its extension, unique per session.
    name: String,
    path: PathBuf,
    header: Entry,
    written: bool,
    last: Option<String>,
}

impl Store {
    pub fn create(paths: &Paths) -> Self {
        let id = entry::new_id();
        let created = io::clock::now();
        let name = format!("{}_{id}", created.strftime("%Y%m%dT%H%M%S"));
        Self {
            path: paths.sessions_dir().join(format!("{name}.jsonl")),
            name,
            header: Entry::Session {
                v: FORMAT_VERSION,
                id,
                created,
                cwd: paths.project.display().to_string(),
                crowbot: release::package().to_owned(),
            },
            written: false,
            last: None,
        }
    }

    /// Whether the file exists yet: the header goes out with the first message.
    pub fn written(&self) -> bool {
        self.written
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the file is, or will be once something is said.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, message: &Message) -> anyhow::Result<()> {
        if !self.written {
            self.write(&self.header)?;
            self.written = true;
        }
        let id = entry::new_id();
        // CEILING: the parent is always the last entry written. Upgrade: undo/retry (M5) passes it in.
        let parent = self.last.take();
        self.write(&Entry::Message {
            id: id.clone(),
            parent,
            ts: io::clock::now(),
            message: message.clone(),
        })?;
        self.last = Some(id);
        Ok(())
    }

    fn write(&self, entry: &Entry) -> anyhow::Result<()> {
        io::fs::append_line(&self.path, &serde_json::to_string(entry)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_header_then_linked_messages() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().join("proj"));
        let mut store = Store::create(&paths);
        assert!(
            !io::fs::exists(store.path()),
            "a session that has said nothing has no file"
        );
        store.append(&Message::user_text("one")).unwrap();
        store.append(&Message::user_text("two")).unwrap();
        let text = io::fs::read_string(store.path()).unwrap().unwrap();
        let entries = entry::parse(&text);
        assert!(matches!(entries[0], Entry::Session { .. }));
        let (Entry::Message { id: first, .. }, Entry::Message { parent, .. }) =
            (&entries[1], &entries[2])
        else {
            panic!("two messages expected");
        };
        assert_eq!(parent.as_ref(), Some(first));
    }
}
