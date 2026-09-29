use crate::agent::message::Message;
use crate::session::store::Store;

/// The conversation in memory, mirrored to its session file when it has one.
pub struct Transcript {
    pub messages: Vec<Message>,
    store: Option<Store>,
}

impl Transcript {
    pub fn new(store: Option<Store>) -> Self {
        Self {
            messages: Vec::new(),
            store,
        }
    }

    /// The session file, when there is one.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.store.as_ref().map(|s| s.path.as_path())
    }

    /// Identifies the session in file names (plans); stable for a stored session.
    pub fn id(&self) -> String {
        self.store
            .as_ref()
            .map_or_else(|| "unsaved".to_owned(), Store::stem)
    }

    pub fn push(&mut self, message: Message) -> anyhow::Result<()> {
        if let Some(store) = &mut self.store {
            store.append(&message)?;
        }
        self.messages.push(message);
        Ok(())
    }
}
