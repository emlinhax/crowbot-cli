//! Where a path argument points, and what touching it needs permission for.

use std::path::{Component, Path, PathBuf};

use crate::permission::gate::Ask;

pub struct Target {
    pub path: PathBuf,
    /// Relative to the project with `/` separators when inside it, else absolute with `/`.
    /// This is what permission rules match and what the user sees.
    pub shown: String,
    pub outside: bool,
}

pub fn resolve(project: &Path, raw: &str) -> Target {
    let raw = msys_to_windows(raw.trim());
    let joined = if Path::new(&raw).is_absolute() {
        PathBuf::from(&raw)
    } else {
        project.join(&raw)
    };
    let path = normalize(&joined);
    let base = normalize(project);
    let (shown, outside) = match path.strip_prefix(&base) {
        Ok(rel) if rel.as_os_str().is_empty() => (".".to_owned(), false),
        Ok(rel) => (slashes(rel), false),
        Err(_) => (slashes(&path), true),
    };
    Target {
        path,
        shown,
        outside,
    }
}

/// The asks for `permission` on this target, plus leaving the project when it does.
pub fn asks(permission: &str, target: &Target) -> Vec<Ask> {
    let mut asks = vec![Ask::new(permission, target.shown.clone())];
    if target.outside {
        asks.push(Ask::new("external_directory", target.shown.clone()));
    }
    asks
}

/// Resolves `.` and `..` without touching the disk (the file may not exist yet).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Prefix(prefix) => {
                // Drive letters differ only in case; make `c:` and `C:` the same place.
                out.push(prefix.as_os_str().to_string_lossy().to_uppercase());
            }
            other => out.push(other),
        }
    }
    out
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
        let t = resolve(&project, "src/../src/./main.rs");
        assert_eq!(t.shown, "src/main.rs");
        assert!(!t.outside);
        let t = resolve(&project, "../other/x.txt");
        assert!(t.outside);
        assert!(t.shown.ends_with("/other/x.txt"), "{}", t.shown);
        assert_eq!(asks("edit", &t).len(), 2);
        assert_eq!(resolve(&project, ".").shown, ".");
    }

    #[cfg(windows)]
    #[test]
    fn msys_and_lowercase_drives_resolve_to_the_same_place() {
        let project = PathBuf::from(r"C:\Users\j\proj");
        assert_eq!(resolve(&project, "/c/Users/j/proj/a.rs").shown, "a.rs");
        assert_eq!(resolve(&project, r"c:\Users\j\proj\a.rs").shown, "a.rs");
    }
}
