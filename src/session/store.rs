use std::path::PathBuf;

use crate::agent::message::Message;
use crate::io;
use crate::paths::Paths;
use crate::session::entry::{self, Entry, FORMAT_VERSION};

/// An append-only JSONL session file; the only writer of its file. Nothing is written until
/// the first message, so a session that never says anything leaves no file.
pub struct Store {
    pub path: PathBuf,
    header: Entry,
    written: bool,
    last: Option<String>,
}

impl Store {
    pub fn create(paths: &Paths) -> Self {
        let id = entry::new_id();
        let created = io::clock::now();
        let stamp = created.strftime("%Y%m%dT%H%M%S");
        Self {
            path: paths.sessions_dir().join(format!("{stamp}_{id}.jsonl")),
            header: Entry::Session {
                v: FORMAT_VERSION,
                id,
                created,
                cwd: paths.project.display().to_string(),
                crowbot: env!("CARGO_PKG_VERSION").to_owned(),
            },
            written: false,
            last: None,
        }
    }

    /// Whether the file exists yet: the header goes out with the first message.
    pub fn written(&self) -> bool {
        self.written
    }

    /// The file name without extension, unique per session.
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
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
            !store.path.exists(),
            "a session that has said nothing has no file"
        );
        store.append(&Message::user_text("one")).unwrap();
        store.append(&Message::user_text("two")).unwrap();
        let text = io::fs::read_string(&store.path).unwrap().unwrap();
        let entries: Vec<Entry> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(entries[0], Entry::Session { .. }));
        let (Entry::Message { id: first, .. }, Entry::Message { parent, .. }) =
            (&entries[1], &entries[2])
        else {
            panic!("two messages expected");
        };
        assert_eq!(parent.as_ref(), Some(first));
    }
}
