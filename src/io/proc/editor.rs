//! The user's own editor, given the terminal to change a file in.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static EDITORS: LazyLock<Editors> = data::load!("editors.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Editors {
    unix: Vec<String>,
    windows: Vec<String>,
}

/// Opens `file` in the first editor found and waits for it to close. The caller lends it the
/// terminal first (`term::Raw::lend`).
pub async fn edit(file: &Path) -> io::Result<()> {
    let none = || io::Error::new(io::ErrorKind::NotFound, "none found");
    let (program, args) = found().ok_or_else(none)?;
    let status = tokio::process::Command::new(&program)
        .args(args)
        .arg(file)
        .status()
        .await?;
    match status.success() {
        true => Ok(()),
        false => Err(io::Error::other(format!("{} {status}", program.display()))),
    }
}

// CEILING: words split on spaces, so a program whose path has a space cannot take arguments.
// Upgrade: shell-style quoting.
fn found() -> Option<(PathBuf, Vec<String>)> {
    let candidates = match cfg!(windows) {
        true => &EDITORS.windows,
        false => &EDITORS.unix,
    };
    candidates.iter().find_map(|candidate| {
        let line = super::filled(candidate)?;
        let mut words = line.split_whitespace();
        let program = super::which(words.next()?)?;
        Some((program, words.map(str::to_owned).collect()))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_os_has_an_editor_to_fall_back_on() {
        let editors = &*super::EDITORS;
        for list in [&editors.unix, &editors.windows] {
            assert!(list.iter().any(|c| !c.contains('{')), "{list:?}");
        }
    }
}
