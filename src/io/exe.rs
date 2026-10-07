//! The program itself: where it runs from, and putting a new build in its place while it may be
//! running.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::{Path, PathBuf};

use super::fs::list_dir;

/// The program this process runs from.
pub fn current() -> io::Result<PathBuf> {
    std::env::current_exe()
}

/// Writes `bytes` as a program (mode 0755 on Unix), over any file already there.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Puts `bytes` where the program `target` is, though it may be running: written to a stage
/// beside it first, so a half-written program never takes its place. Any failure removes the
/// stage.
pub fn stage_and_replace(bytes: &[u8], target: &Path) -> io::Result<()> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let stage = target.with_file_name(format!(".{name}.new"));
    let swapped = write(&stage, bytes).and_then(|()| replace(&stage, target));
    if swapped.is_err() {
        let _ = std::fs::remove_file(&stage);
    }
    swapped
}

/// Unix renames over `current` (a running process keeps its image). Windows cannot replace a
/// running exe but can rename it, so the old one moves aside and is swept on a later start.
fn replace(new: &Path, current: &Path) -> io::Result<()> {
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

/// Removes the programs earlier replacements of `current` moved aside (`*.old-*`); one still
/// running cannot be removed and stays for a later start.
pub fn sweep(current: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fs::read_bytes;

    #[test]
    fn a_program_is_replaced_where_it_stands_and_its_leftovers_swept() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(if cfg!(windows) {
            "crowbot.exe"
        } else {
            "crowbot"
        });
        write(&current, b"old").unwrap();
        assert!(writable(dir.path()));
        stage_and_replace(b"new", &current).unwrap();
        assert_eq!(read_bytes(&current).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&current).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        sweep(&current);
        let left: Vec<String> = list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    // Windows moves whatever is there aside, so only Unix fails this rename.
    #[cfg(unix)]
    #[test]
    fn a_failed_swap_leaves_no_stage() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the program should be: the stage is written, the rename fails.
        let target = dir.path().join("crowbot");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), "x").unwrap();
        assert!(stage_and_replace(b"new", &target).is_err());
        let left: Vec<String> = list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(left, ["crowbot"]);
    }
}
