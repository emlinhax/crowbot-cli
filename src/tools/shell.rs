//! Which shell the bash tool runs commands in, chosen from data/shells.toml.

use std::path::PathBuf;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::io;
use crate::settings;
use crate::text::template;

const SRC: &str = include_str!("../../data/shells.toml");

static CATALOG: LazyLock<Catalog> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/shells.toml is checked by tests"));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    shell: Vec<Entry>,
    env: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    os: String,
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
}

pub fn resolve(configured: Option<&str>) -> Shell {
    let os = if cfg!(windows) { "windows" } else { "unix" };
    let env: Vec<(String, String)> = CATALOG
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let entries: Vec<&Entry> = CATALOG.shell.iter().filter(|e| e.os == os).collect();

    if let Some(configured) = configured {
        // A configured shell borrows the arguments of the catalog entry it resembles.
        let stem = std::path::Path::new(configured)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let like = entries.iter().find(|e| {
            e.candidates.iter().any(|c| {
                std::path::Path::new(c)
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem)
            })
        });
        return Shell {
            name: configured.to_owned(),
            program: io::proc::which(configured),
            args: like.map_or_else(|| vec!["-c".to_owned()], |e| e.args.clone()),
            note: like.map_or_else(
                || format!("Commands run in {configured}."),
                |e| e.note.clone(),
            ),
            env,
        };
    }

    for entry in &entries {
        for candidate in &entry.candidates {
            let vars: Vec<(String, String)> = ["LOCALAPPDATA", "ProgramFiles", "HOME"]
                .iter()
                .filter_map(|v| settings::env(v).map(|value| ((*v).to_owned(), value)))
                .collect();
            let refs: Vec<(&str, &str)> =
                vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let path = template::fill(candidate, &refs);
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
                };
            }
        }
    }
    Shell {
        name: "none".into(),
        program: None,
        args: Vec::new(),
        note: "No shell was found on this machine; commands cannot run.".into(),
        env,
    }
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
}
