//! Files the model has read, so it cannot overwrite one it never read or that changed since; and a
//! lock per file so parallel edits land in turn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use serde::Deserialize;

use crate::data;
use crate::io;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = data::load!("tools/files.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    unread: String,
    stale: String,
}

#[derive(Default)]
pub struct Files {
    seen: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl Files {
    /// Forgets every file, for a conversation that has read none yet.
    pub fn forget_all(&self) {
        self.seen.lock().unwrap().clear();
    }

    /// Remembers the file as the model now knows it.
    pub fn saw(&self, path: &Path) {
        let mtime = io::fs::modified(path);
        self.seen.lock().unwrap().insert(path.to_path_buf(), mtime);
    }

    /// An existing file may only be changed after it was read, and only if unchanged since.
    pub fn check_fresh(&self, path: &Path, shown: &str) -> Result<(), String> {
        let Some(current) = io::fs::modified(path) else {
            return Ok(());
        };
        match self.seen.lock().unwrap().get(path) {
            None => Err(fill(&TEXT.unread, &[("path", shown)])),
            Some(seen) if *seen != Some(current) => Err(fill(&TEXT.stale, &[("path", shown)])),
            Some(_) => Ok(()),
        }
    }

    /// The file's lock, once it is held and the file may be changed; held until the guard drops.
    pub async fn claim(
        &self,
        path: &Path,
        shown: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        let guard = self.lock(path).await;
        self.check_fresh(path, shown)?;
        Ok(guard)
    }

    pub async fn lock(&self, path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .locks
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default()
            .clone();
        lock.lock_owned().await
    }
}
