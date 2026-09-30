//! Where a path argument points, and what touching it needs permission for.
//! CEILING: resolved when the call is checked; a link swapped in before it runs is followed.
//! The upgrade is opening each component without following links (openat2 RESOLVE_BENEATH).

use std::path::{Component, Path, PathBuf};

use super::permissions;
use crate::io;
use crate::permission::gate::Ask;

pub struct Target {
    /// The real path, with links resolved: what the tool reads or writes.
    pub path: PathBuf,
    /// Relative to the project with `/` separators when inside it, else absolute with `/`.
    /// This is what permission rules match and what the user sees.
    pub shown: String,
    pub outside: bool,
}

impl Target {
    /// What the path is, or why that cannot be told, in words for the model.
    pub fn kind(&self) -> Result<io::fs::Kind, String> {
        io::fs::kind(&self.path).map_err(|e| format!("Could not look at {}: {e}", self.shown))
    }
}

/// Links are followed before deciding inside or outside, so a link out of the project asks to
/// leave it and a link to `.env` is matched as `.env`.
pub fn resolve(project: &Path, raw: &str) -> Result<Target, String> {
    let raw = msys_to_windows(raw.trim());
    let raw = Path::new(&raw);
    if raw.components().next() == Some(Component::Normal(std::ffi::OsStr::new("~"))) {
        return Err("`~` is not expanded here; use an absolute path.".into());
    }
    if cfg!(windows)
        && raw
            .components()
            .any(|c| matches!(c, Component::Normal(part) if part.to_string_lossy().contains(':')))
    {
        return Err("`:` names a stream, not a file; use a plain path.".into());
    }
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        project.join(raw)
    };
    let path = io::fs::canonical(&joined);
    let (shown, outside) = show(&io::fs::canonical(project), &path);
    Ok(Target {
        path,
        shown,
        outside,
    })
}

/// How `path` is shown and matched, given the resolved project root, and whether it lies outside.
pub fn show(base: &Path, path: &Path) -> (String, bool) {
    match path.strip_prefix(base) {
        Ok(rel) if rel.as_os_str().is_empty() => (".".to_owned(), false),
        Ok(rel) => (slashes(rel), false),
        Err(_) => (slashes(path), true),
    }
}

/// The asks for `permission` on this target, plus leaving the project when it does.
pub fn asks(permission: &str, target: &Target) -> Vec<Ask> {
    let mut asks = vec![Ask::new(permission, target.shown.clone())];
    if target.outside {
        asks.push(Ask::new(
            permissions::EXTERNAL_DIRECTORY.name,
            target.shown.clone(),
        ));
    }
    asks
}

/// Git Bash hands out `/c/Users/...`; Windows wants `C:/Users/...`.
fn msys_to_windows(raw: &str) -> String {
    if cfg!(windows) {
        let bytes = raw.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
        {
            return format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &raw[2..]);
        }
    }
    raw.to_owned()
}

fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_paths_are_relative_and_outside_ones_absolute() {
        let project = std::env::temp_dir().join("proj");
        let t = resolve(&project, "src/../src/./main.rs").unwrap();
        assert_eq!(t.shown, "src/main.rs");
        assert!(!t.outside);
        let t = resolve(&project, "../other/x.txt").unwrap();
        assert!(t.outside);
        assert!(t.shown.ends_with("/other/x.txt"), "{}", t.shown);
        assert_eq!(asks("edit", &t).len(), 2);
        assert_eq!(resolve(&project, ".").unwrap().shown, ".");
    }

    #[test]
    fn a_leading_tilde_is_refused_not_taken_as_a_folder() {
        let project = std::env::temp_dir().join("proj");
        assert!(resolve(&project, "~/.bashrc").is_err());
        assert!(resolve(&project, "notes/~draft").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_followed_before_deciding_inside_or_outside() {
        let root = tempfile::tempdir().unwrap();
        let (project, away) = (root.path().join("proj"), root.path().join("away"));
        for (file, text) in [(project.join(".env"), "KEY=1"), (away.join("x.txt"), "x")] {
            io::fs::write_atomic(&file, text.as_bytes(), io::fs::Access::Shared).unwrap();
        }
        std::os::unix::fs::symlink(away.join("x.txt"), project.join("file")).unwrap();
        std::os::unix::fs::symlink(&away, project.join("dir")).unwrap();
        std::os::unix::fs::symlink(project.join(".env"), project.join("notes")).unwrap();

        let file = resolve(&project, "file").unwrap();
        assert!(file.outside, "{}", file.shown);
        assert_eq!(file.path, io::fs::canonical(&away.join("x.txt")));
        let dir = resolve(&project, "dir/new.txt").unwrap();
        assert!(dir.outside, "{}", dir.shown);
        let alias = resolve(&project, "notes").unwrap();
        assert_eq!(alias.shown, ".env");
        assert!(!alias.outside);
    }

    #[cfg(windows)]
    #[test]
    fn msys_and_lowercase_drives_resolve_to_the_same_place() {
        let project = PathBuf::from(r"C:\Users\j\proj");
        assert_eq!(
            resolve(&project, "/c/Users/j/proj/a.rs").unwrap().shown,
            "a.rs"
        );
        assert_eq!(
            resolve(&project, r"c:\Users\j\proj\a.rs").unwrap().shown,
            "a.rs"
        );
        assert!(resolve(&project, "a.rs:hidden").is_err());
    }
}
