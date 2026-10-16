use std::path::{Path, PathBuf};

use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::io;
use crate::release;
use crate::session::entry::{self, Entry, FORMAT_VERSION, Snap, State};

/// Append-only JSONL session file and its only writer. Nothing is written before the first message.
pub struct Store {
    /// `{stamp}_{id}`: the file's name without its extension, unique per session.
    name: String,
    path: PathBuf,
    /// Written with the first message; `None` once it is in the file.
    header: Option<Entry>,
    /// The entry the next message follows.
    head: Option<String>,
}

impl Store {
    pub fn create(paths: &Paths) -> Self {
        let id = entry::new_id();
        let created = io::clock::now();
        let name = format!("{}_{id}", created.strftime("%Y%m%dT%H%M%S"));
        Self {
            path: paths.sessions_dir().join(format!("{name}.jsonl")),
            name,
            header: Some(Entry::Session {
                v: FORMAT_VERSION,
                id,
                created,
                cwd: paths.project.display().to_string(),
                crowbot: release::package().to_owned(),
            }),
            head: None,
        }
    }

    /// An existing file, to go on from `head`, the last entry of its current branch.
    pub fn open(path: PathBuf, head: Option<String>) -> Self {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            name,
            path,
            header: None,
            head,
        }
    }

    /// Whether the file exists yet: the header goes out with the first message.
    pub fn written(&self) -> bool {
        self.header.is_none()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the file is, or will be once something is said.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `message` as entry `id`, following the head, which it then becomes.
    pub fn append(
        &mut self,
        id: &str,
        message: &Message,
        state: Option<State>,
    ) -> anyhow::Result<()> {
        if let Some(header) = &self.header {
            self.write(header)?;
            self.header = None;
        }
        let parent = self.head.replace(id.to_owned());
        self.write(&Entry::Message {
            id: id.to_owned(),
            parent,
            ts: io::clock::now(),
            message: message.clone(),
            state,
        })
    }

    /// Records prompt `of`'s snapshots; the head stays where it is.
    pub fn snapshot(&mut self, of: &str, trees: &Snap) -> anyhow::Result<()> {
        self.write(&Entry::Snapshot {
            ts: io::clock::now(),
            of: of.to_owned(),
            trees: trees.clone(),
        })
    }

    /// Moves the head back to entry `to` (`None`: before the first message). What followed it
    /// stays in the file, off the current branch.
    pub fn rewind(&mut self, to: Option<&str>) -> anyhow::Result<()> {
        if !self.written() {
            return Ok(());
        }
        let from = std::mem::replace(&mut self.head, to.map(str::to_owned));
        self.write(&Entry::Rewind {
            ts: io::clock::now(),
            to: to.map(str::to_owned),
            from,
        })
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
        store.append("a", &Message::user_text("one"), None).unwrap();
        store.append("b", &Message::user_text("two"), None).unwrap();
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
