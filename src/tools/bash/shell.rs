//! Which shell the bash tool runs commands in, chosen from data/shells.toml.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::io;
use crate::text::template;

static CATALOG: LazyLock<Catalog> = data::load!("shells.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    shell: Vec<Entry>,
    fallback: Fallback,
    unset: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fallback {
    name: String,
    args: Vec<String>,
    configured: String,
    missing: String,
    none: String,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Os {
    Windows,
    Unix,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    os: Os,
    name: String,
    candidates: Vec<String>,
    args: Vec<String>,
    note: String,
}

#[derive(Clone, Debug)]
pub struct Shell {
    pub name: String,
    /// `None` when nothing was found; commands then fail with a clear message.
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    pub note: String,
    pub env: Vec<(String, String)>,
    /// Variables the command must not inherit.
    pub unset: Vec<String>,
}

pub fn resolve(configured: Option<&str>) -> Shell {
    let os = if cfg!(windows) { Os::Windows } else { Os::Unix };
    let fallback = &CATALOG.fallback;
    let env: Vec<(String, String)> = CATALOG
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let unset = CATALOG.unset.clone();
    let entries: Vec<&Entry> = CATALOG.shell.iter().filter(|e| e.os == os).collect();

    if let Some(configured) = configured {
        // A configured shell borrows the arguments of the catalog entry it resembles.
        let like = entries
            .iter()
            .find(|e| e.candidates.iter().any(|c| stem(c) == stem(configured)));
        let program = io::proc::which(configured);
        let note = match (&program, like) {
            (None, _) => &fallback.missing,
            (Some(_), Some(entry)) => &entry.note,
            (Some(_), None) => &fallback.configured,
        };
        return Shell {
            name: configured.to_owned(),
            program,
            args: like.map_or_else(|| fallback.args.clone(), |e| e.args.clone()),
            note: template::fill(note, &[("shell", configured)]),
            env,
            unset,
        };
    }

    for entry in &entries {
        for candidate in &entry.candidates {
            let path = template::fill_with(candidate, |name| io::env::var(name).map(Cow::Owned));
            if path.contains('{') {
                continue;
            }
            if let Some(program) = io::proc::which(&path) {
                return Shell {
                    name: entry.name.clone(),
                    program: Some(program),
                    args: entry.args.clone(),
                    note: entry.note.clone(),
                    env,
                    unset,
                };
            }
        }
    }
    Shell {
        name: fallback.name.clone(),
        program: None,
        args: Vec::new(),
        note: fallback.none.clone(),
        env,
        unset,
    }
}

/// A program's name without its folder or extension, lowercased: `C:/x/Bash.EXE` is `bash`.
fn stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_a_shell_is_found_here() {
        assert!(CATALOG.shell.iter().all(|e| !e.name.is_empty()));
        let shell = resolve(None);
        assert!(shell.program.is_some(), "{}", shell.note);
    }

    #[test]
    fn commands_never_inherit_crowbots_key() {
        assert!(CATALOG.unset.iter().any(|v| v == crate::auth::ENV_VAR));
        assert_eq!(resolve(None).unset, CATALOG.unset);
    }

    #[test]
    fn a_configured_shell_that_is_missing_says_so() {
        let shell = resolve(Some("/nowhere/zsh"));
        assert!(shell.program.is_none());
        assert_eq!(
            shell.note,
            "The configured shell /nowhere/zsh was not found; commands cannot run."
        );
    }
}
