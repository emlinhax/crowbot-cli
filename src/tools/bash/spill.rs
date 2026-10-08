//! Where a command's output goes while it runs: memory while small, then a spill file.

use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::config::limits;
use crate::io::{self, fs::Access};

/// What a command printed, within bounds: all of it while small, then only the latest part in
/// memory and the whole, up to a cap, in a spill file the model can read.
pub struct Sink {
    pub held: Vec<u8>,
    pub spill: Option<Spill>,
    spill_dir: PathBuf,
    newlines: usize,
    ends_mid_line: bool,
}

pub struct Spill {
    file: io::fs::Appender,
    pub path: PathBuf,
    written: usize,
    /// The cap was reached; the file holds only the start.
    pub stopped: bool,
}

impl Sink {
    pub fn new(spill_dir: PathBuf) -> Self {
        Self {
            held: Vec::new(),
            spill: None,
            spill_dir,
            newlines: 0,
            ends_mid_line: false,
        }
    }

    pub fn take(&mut self, chunk: &[u8]) {
        self.newlines += chunk.iter().filter(|&&b| b == b'\n').count();
        if let Some(&last) = chunk.last() {
            self.ends_mid_line = last != b'\n';
        }
        if let Some(spill) = &mut self.spill {
            spill.write(chunk);
        }
        self.held.extend_from_slice(chunk);
        let keep = limits::get().tools.bash_memory_bytes.value;
        if self.held.len() > keep {
            if self.spill.is_none() {
                self.spill = Spill::open(&self.spill_dir, &self.held);
            }
            let excess = self.held.len() - keep;
            self.held.drain(..excess);
        }
    }

    pub fn lines(&self) -> usize {
        self.newlines + usize::from(self.ends_mid_line)
    }
}

impl Spill {
    pub fn open(dir: &Path, start: &[u8]) -> Option<Self> {
        let (path, file) = create_spill(dir)?;
        let mut spill = Self {
            file,
            path,
            written: 0,
            stopped: false,
        };
        spill.write(start);
        Some(spill)
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.stopped {
            return;
        }
        let room = limits::get().tools.bash_spill_max_bytes.value - self.written;
        let part = &bytes[..bytes.len().min(room)];
        self.stopped = part.len() < bytes.len() || self.file.append(part).is_err();
        self.written += part.len();
    }
}

/// Spill files are `bash-<8 hex digits>.log`; pruning touches only names of that form.
const PREFIX: &str = "bash-";
const SUFFIX: &str = ".log";

static PRUNED: Once = Once::new();

/// A new spill file, never one an older transcript still points to. The process's first spill
/// also clears out old ones.
fn create_spill(dir: &Path) -> Option<(PathBuf, io::fs::Appender)> {
    PRUNED.call_once(|| prune_spills(dir));
    // 32 random bits rarely collide; a few draws make it never in practice.
    for _ in 0..8 {
        let path = dir.join(format!("{PREFIX}{:08x}{SUFFIX}", fastrand::u32(..)));
        match io::fs::Appender::create(&path, Access::Shared) {
            Ok(file) => return Some((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

fn prune_spills(dir: &Path) {
    let keep = limits::get().tools.bash_spill_keep_days.signed_days();
    let now = io::clock::now();
    for entry in io::fs::list_dir(dir).unwrap_or_default() {
        if !(entry.name.starts_with(PREFIX) && entry.name.ends_with(SUFFIX)) {
            continue;
        }
        let path = dir.join(&entry.name);
        let age = io::fs::modified(&path)
            .and_then(|m| jiff::Timestamp::try_from(m).ok())
            .map(|m| now.duration_since(m));
        if age.is_some_and(|age| age > keep) {
            let _ = io::fs::remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ageing a file is something only this test does, so it reaches the filesystem itself.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn old_spills_are_pruned_and_fresh_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (old, fresh, other) = ("bash-00000001.log", "bash-00000002.log", "notes.log");
        for name in [old, fresh, other] {
            io::fs::write_atomic(&dir.path().join(name), b"x", Access::Shared).unwrap();
        }
        for name in [old, other] {
            std::fs::File::options()
                .write(true)
                .open(dir.path().join(name))
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000))
                .unwrap();
        }
        prune_spills(dir.path());
        let left: Vec<String> = io::fs::list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(left, vec![fresh, other]);
    }

    #[test]
    fn a_spill_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let names: std::collections::HashSet<_> = (0..50)
            .map(|_| create_spill(dir.path()).unwrap().0)
            .collect();
        assert_eq!(names.len(), 50);
    }
}
