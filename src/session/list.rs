//! This project's sessions on disk.

use std::path::PathBuf;

use crate::config::paths::Paths;
use crate::io;

/// The session file written to last, if this project has one.
pub fn latest(paths: &Paths) -> Option<PathBuf> {
    let dir = paths.sessions_dir();
    io::fs::list_dir(&dir)
        .ok()?
        .into_iter()
        .filter(|e| !e.is_dir && e.name.ends_with(".jsonl"))
        .map(|e| dir.join(e.name))
        .max_by_key(|path| io::fs::modified(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use crate::session::transcript::Transcript;

    #[test]
    fn the_latest_is_the_one_written_to_last() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        assert_eq!(latest(&paths), None);
        let mut older = Transcript::create(&paths);
        older.push(Message::user_text("one")).unwrap();
        let mut newer = Transcript::create(&paths);
        newer.push(Message::user_text("two")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        older.push(Message::user_text("three")).unwrap();
        assert_eq!(latest(&paths).as_deref(), older.path());
    }
}
