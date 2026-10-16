//! Where a command line runs: a command that names the project as its directory is read as if it
//! named nothing (data/workdir.toml), so `cd <project> && cargo test` asks about `cargo test`.

use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static FILE: LazyLock<File> = data::load!("workdir.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    moves: Vec<String>,
    strip: Vec<Strip>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Strip {
    words: Vec<String>,
    keep: usize,
}

/// `command` (one command of a split line) as the gate should read it, or `None` when naming the
/// project was all it did.
pub fn strip(command: &str, project: &Path) -> Option<String> {
    let words = words(command);
    let form = FILE.strip.iter().find(|form| {
        form.words.len() <= words.len()
            && form
                .words
                .iter()
                .zip(&words)
                .all(|(want, (got, _))| match want.as_str() {
                    "{dir}" => is_project(got, project),
                    want => want == got,
                })
    });
    let Some(form) = form else {
        return Some(command.to_owned());
    };
    let rest = command[words[form.words.len() - 1].1..].trim_start();
    let kept = words[..form.keep].iter().map(|(w, _)| w.as_str());
    let left: Vec<&str> = kept.chain([rest]).filter(|w| !w.is_empty()).collect();
    (!left.is_empty()).then(|| left.join(" "))
}

/// Whether the commands after `command` run somewhere else.
pub fn moves(command: &str) -> bool {
    words(command)
        .first()
        .is_some_and(|(first, _)| FILE.moves.contains(first))
}

fn is_project(dir: &str, project: &Path) -> bool {
    matches!(dir, "." | "./") || Path::new(dir) == project
}

/// Each word with its quotes removed, and the byte offset just past it.
fn words(command: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut quoted = false;
    for (i, c) in command.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                quoted = true;
            }
            (None, c) if c.is_whitespace() => {
                if quoted || !word.is_empty() {
                    out.push((std::mem::take(&mut word), i));
                }
                quoted = false;
            }
            (None, c) => word.push(c),
        }
    }
    if quoted || !word.is_empty() {
        out.push((word, command.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_the_project_as_the_directory_is_read_as_naming_nothing() {
        let project = Path::new("/tmp/my project");
        let strip = |c: &str| strip(c, project);
        assert_eq!(strip("cd '/tmp/my project'"), None);
        assert_eq!(strip("cd \"/tmp/my project/\""), None);
        assert_eq!(strip("cd ."), None);
        assert_eq!(
            strip("git -C '/tmp/my project' log --oneline -5").as_deref(),
            Some("git log --oneline -5")
        );
        assert_eq!(strip("git -C . status").as_deref(), Some("git status"));
        for elsewhere in [
            "cd /tmp",
            "cd src",
            "git -C /etc log",
            "cd /tmp/my",
            "git log",
        ] {
            assert_eq!(strip(elsewhere).as_deref(), Some(elsewhere));
        }
        assert!(moves("cd src") && moves("pushd /tmp"));
        assert!(!moves("cdx") && !moves("git -C x log"));
    }
}
