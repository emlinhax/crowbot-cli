#![allow(clippy::disallowed_methods)]

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

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
    ensure_parent(path)?;
    let (tmp, file) = create_temp(path, access)?;
    let written = fill(file, bytes, path, access).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// A new file beside `path`, never one already there: not a leftover, not a planted link.
fn create_temp(path: &Path, access: Access) -> io::Result<(PathBuf, std::fs::File)> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    loop {
        let tmp = path.with_file_name(format!(
            "{name}.tmp-{}-{:08x}",
            std::process::id(),
            fastrand::u32(..)
        ));
        match open_new(&tmp, access) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

fn fill(mut file: std::fs::File, bytes: &[u8], path: &Path, access: Access) -> io::Result<()> {
    // A replaced file keeps its mode bits (a script stays executable), set before any content
    // so the new one is never readable more widely than the old.
    if access == Access::Shared
        && let Ok(old) = std::fs::metadata(path)
    {
        file.set_permissions(old.permissions())?;
    }
    file.write_all(bytes)?;
    file.sync_all()
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) => std::fs::create_dir_all(dir),
        None => Ok(()),
    }
}

/// `path` with links, `.` and `..` resolved as far as it exists on disk; the part that does not
/// exist yet (a file about to be written) is appended as written.
pub fn canonical(path: &Path) -> PathBuf {
    let parts: Vec<Component> = path.components().collect();
    for split in (1..=parts.len()).rev() {
        let head: PathBuf = parts[..split].iter().collect();
        if let Ok(real) = std::fs::canonicalize(&head) {
            let mut out = without_verbatim(real);
            for part in &parts[split..] {
                match part {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::CurDir => {}
                    other => out.push(other),
                }
            }
            return out;
        }
    }
    path.to_path_buf()
}

/// Windows answers `\\?\C:\…`; everything else here spells paths `C:\…`.
fn without_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path
    }
}

/// Whether both paths name one existing file, with links and `..` resolved.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Missing,
    File,
    Dir,
}

pub fn kind(path: &Path) -> Kind {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Kind::Dir,
        Ok(_) => Kind::File,
        Err(_) => Kind::Missing,
    }
}

/// Last modification time, or `None` if the file is gone.
pub fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// A directory's entries, sorted by name.
pub fn list_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|e| Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e.file_type().is_ok_and(|t| t.is_dir()),
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Appends one line, creating the file and its directory on first use.
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    ensure_parent(path)?;
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
    options.write(true).create_new(true);
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

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.sh");
        write_atomic(&path, b"echo 1", Access::Shared).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&path, b"echo 2", Access::Shared).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn a_failed_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("taken");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(target.join("inside")).unwrap();
        assert!(write_atomic(&target, b"x", Access::Shared).is_err());
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "{left:?}");
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
