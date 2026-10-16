//! The user's PATH as text: adding crowbot's folder once, taking it off again, however each
//! platform spells its entries.

use std::io;
use std::path::Path;

use crate::io::registry;

/// How a platform writes PATH: the separator, and whether case tells entries apart.
#[derive(Clone, Copy)]
struct Style {
    sep: char,
    ignore_case: bool,
}

const HERE: Style = if cfg!(windows) {
    Style {
        sep: ';',
        ignore_case: true,
    }
} else {
    Style {
        sep: ':',
        ignore_case: false,
    }
};

/// What adding a folder to PATH came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Already,
    /// Not on it, and only the user's shell profile can put it there.
    ShellProfile,
}

/// Puts `dir` on the user's PATH for terminals opened from now on.
pub fn add(dir: &Path) -> io::Result<Change> {
    match with_dir(&registry::user_path()?, &dir.to_string_lossy(), HERE) {
        None => Ok(Change::Already),
        Some(path) if registry::WRITES_PATH => {
            registry::set_user_path(&path).map(|()| Change::Added)
        }
        Some(_) => Ok(Change::ShellProfile),
    }
}

/// Takes `dir` off the user's PATH; `false` when crowbot did not have it there.
pub fn remove(dir: &Path) -> io::Result<bool> {
    match without_dir(&registry::user_path()?, &dir.to_string_lossy(), HERE) {
        Some(path) if registry::WRITES_PATH => registry::set_user_path(&path).map(|()| true),
        _ => Ok(false),
    }
}

/// `path` with `dir` at its end, or `None` when `dir` is already on it.
fn with_dir(path: &str, dir: &str, style: Style) -> Option<String> {
    let entries: Vec<&str> = entries(path, style).collect();
    if entries.iter().any(|e| same(e, dir, style)) {
        return None;
    }
    let joined: Vec<&str> = entries.into_iter().chain([dir]).collect();
    Some(joined.join(&style.sep.to_string()))
}

/// `path` without `dir`, or `None` when `dir` was not on it.
fn without_dir(path: &str, dir: &str, style: Style) -> Option<String> {
    let entries: Vec<&str> = entries(path, style).collect();
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|e| !same(e, dir, style))
        .collect();
    (kept.len() < entries.len()).then(|| kept.join(&style.sep.to_string()))
}

fn entries(path: &str, style: Style) -> impl Iterator<Item = &str> {
    path.split(style.sep).filter(|e| !e.trim().is_empty())
}

/// The same folder, however its trailing separator or (on Windows) its case is written.
fn same(a: &str, b: &str, style: Style) -> bool {
    let trim = |s: &str| s.trim().trim_end_matches(['/', '\\']).to_owned();
    let (a, b) = (trim(a), trim(b));
    if style.ignore_case {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: Style = Style {
        sep: ';',
        ignore_case: true,
    };
    const UNIX: Style = Style {
        sep: ':',
        ignore_case: false,
    };

    #[test]
    fn a_folder_joins_the_path_once() {
        let path = r"C:\Windows;%USERPROFILE%\bin;";
        let dir = r"C:\Users\j\AppData\Local\Programs\crowbot";
        assert_eq!(
            with_dir(path, dir, WINDOWS).unwrap(),
            format!(r"C:\Windows;%USERPROFILE%\bin;{dir}")
        );
        let written = with_dir(path, dir, WINDOWS).unwrap();
        assert_eq!(with_dir(&written, &dir.to_uppercase(), WINDOWS), None);
        assert_eq!(with_dir(&format!("{written}\\"), dir, WINDOWS), None);
        assert_eq!(
            with_dir("", "/home/j/.local/bin", UNIX).unwrap(),
            "/home/j/.local/bin"
        );
        assert!(with_dir("/usr/bin:/HOME/j/.local/bin", "/home/j/.local/bin", UNIX).is_some());
    }

    #[test]
    fn a_folder_leaves_the_path_and_nothing_else_does() {
        let dir = r"C:\Users\j\AppData\Local\Programs\crowbot";
        let path = format!(r"C:\Windows;{dir}\;%USERPROFILE%\bin");
        assert_eq!(
            without_dir(&path, dir, WINDOWS).unwrap(),
            r"C:\Windows;%USERPROFILE%\bin"
        );
        assert_eq!(without_dir(r"C:\Windows", dir, WINDOWS), None);
    }
}
