#![allow(clippy::disallowed_methods)]

use std::io::{self, Write};
use std::path::Path;

/// Who may read a written file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Shared,
    /// Owner only (0600 on Unix); for keys. Windows files inherit the user profile's ACL.
    Private,
}

/// `Ok(None)` when the file does not exist, so callers treat "absent" as a normal case.
pub fn read_string(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes a sibling temp file and renames it over the target, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8], access: Access) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = open_new(&tmp, access)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Appends one line, creating the file and its directory on first use.
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{line}\n").as_bytes())
}

/// `Ok(false)` when there was nothing to remove.
pub fn remove(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn open_new(path: &Path, access: Access) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = access;
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_string(&dir.path().join("nope.txt")).unwrap(), None);
    }

    #[test]
    fn atomic_write_creates_parents_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.json");
        write_atomic(&path, b"one", Access::Shared).unwrap();
        write_atomic(&path, b"two", Access::Private).unwrap();
        assert_eq!(read_string(&path).unwrap().as_deref(), Some("two"));
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_atomic(&path, b"k", Access::Private).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn append_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s/log.jsonl");
        append_line(&path, "a").unwrap();
        append_line(&path, "b").unwrap();
        assert_eq!(read_string(&path).unwrap().as_deref(), Some("a\nb\n"));
        assert!(remove(&path).unwrap());
        assert!(!remove(&path).unwrap());
    }
}
