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

/// What is at `path`, following links. Only absence is `Missing`; an error that leaves it
/// unknown (no permission, say) is returned as the error it is.
pub fn kind(path: &Path) -> io::Result<Kind> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Ok(Kind::Dir),
        Ok(_) => Ok(Kind::File),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(Kind::Missing)
        }
        Err(e) => Err(e),
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

/// A directory's entries, sorted by name; a link to a directory counts as one.
pub fn list_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|e| Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e
                .file_type()
                .is_ok_and(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())),
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// A file created for appending, never one that was already there.
pub struct Appender(std::fs::File);

impl Appender {
    pub fn create(path: &Path, access: Access) -> io::Result<Self> {
        ensure_parent(path)?;
        open_new(path, access).map(Self)
    }

    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }
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

/// Writes `bytes` as a program (mode 0755 on Unix), over any file already there.
pub fn write_executable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub fn make_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Removes `dir` and all it holds; absent is fine. Only Windows installs into a folder of
/// crowbot's own.
#[cfg(windows)]
pub fn remove_dir(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Whether a file can be made in `dir`.
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".crowbot-probe-{}", std::process::id()));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if made {
        let _ = std::fs::remove_file(&probe);
    }
    made
}

/// Puts the program `new` where `current` is, though `current` may be running. Unix renames over
/// it (a running process keeps its image). Windows cannot replace a running exe but can rename
/// it, so the old one moves aside and is swept on a later start.
pub fn replace_executable(new: &Path, current: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if !current.exists() {
            return std::fs::rename(new, current);
        }
        let aside = current.with_extension(format!("old-{}", std::process::id()));
        std::fs::rename(current, &aside)?;
        if let Err(e) = std::fs::rename(new, current) {
            let _ = std::fs::rename(&aside, current);
            return Err(e);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    std::fs::rename(new, current)
}

/// Removes what earlier replacements of `current` left beside it: renamed-aside programs (some
/// may still run, and stay) and staged downloads.
pub fn sweep_replaced(current: &Path) {
    let (Some(dir), Some(stem)) = (current.parent(), current.file_stem()) else {
        return;
    };
    let old = format!("{}.old-", stem.to_string_lossy());
    for entry in list_dir(dir).unwrap_or_default() {
        if entry.name.starts_with(&old) {
            let _ = std::fs::remove_file(dir.join(&entry.name));
        }
    }
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

    #[cfg(unix)]
    #[test]
    fn a_path_that_cannot_be_looked_at_is_not_called_missing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("a.txt"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = kind(&locked.join("a.txt"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: a plain query. Root reads through permissions, so there it is simply a file.
        let root = unsafe { libc::geteuid() } == 0;
        assert!(
            root || found
                .as_ref()
                .is_err_and(|e| e.kind() == io::ErrorKind::PermissionDenied),
            "{found:?}"
        );
        assert_eq!(kind(&locked.join("gone")).unwrap(), Kind::Missing);
        assert_eq!(kind(&locked.join("a.txt/b")).unwrap(), Kind::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_directory_lists_as_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let dirs: Vec<bool> = list_dir(dir.path())
            .unwrap()
            .iter()
            .map(|e| e.is_dir)
            .collect();
        assert_eq!(dirs, [true, true]);
    }

    #[test]
    fn a_program_is_replaced_where_it_stands_and_its_leftovers_swept() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(if cfg!(windows) {
            "crowbot.exe"
        } else {
            "crowbot"
        });
        write_executable(&current, b"old").unwrap();
        let new = dir.path().join(".crowbot.new");
        write_executable(&new, b"new").unwrap();
        assert!(writable(dir.path()));
        replace_executable(&new, &current).unwrap();
        assert_eq!(read_bytes(&current).unwrap(), b"new");
        assert!(!new.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&current).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        sweep_replaced(&current);
        let left: Vec<String> = list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

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
