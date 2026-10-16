//! Folders whose .crowbot/config.toml may loosen settings. The list lives in crowbot's home, out of
//! a repository's reach.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::config::paths::Paths;
use crate::data;
use crate::io;
use crate::text::template;

static DATA: LazyLock<Data> = data::load!("trust.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    untrusted_keys: Vec<String>,
    masked: Vec<String>,
    mask: String,
    warning: String,
    danger: String,
    question: String,
    declined: String,
    ignored: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    projects: Vec<String>,
}

/// A corrupt list is an error, not an empty one: this file is a security gate.
fn load(paths: &Paths) -> anyhow::Result<Store> {
    Ok(io::fs::read_json(&paths.trusted())?.unwrap_or_default())
}

fn project(paths: &Paths) -> String {
    paths.project.display().to_string()
}

/// What an untrusted project's config asked for and did not get.
#[derive(Debug)]
pub struct Untrusted {
    pub file: PathBuf,
    /// Each setting left out, as written: `mode = "auto"`.
    pub keys: Vec<String>,
}

/// The part of `file`, a project config layer, this folder may apply: all of it once trusted,
/// else only what cannot loosen anything, with what was left out.
pub fn admit(
    paths: &Paths,
    file: &Path,
    layer: toml::Table,
) -> anyhow::Result<(toml::Table, Option<Untrusted>)> {
    if is_trusted(paths)? {
        return Ok((layer, None));
    }
    let (safe, keys) = split_untrusted(layer);
    let untrusted = (!keys.is_empty()).then(|| Untrusted {
        file: file.to_path_buf(),
        keys,
    });
    Ok((safe, untrusted))
}

/// Deny rules may come from an untrusted folder too: under last-match-wins an appended deny can
/// only tighten, while an `ask` could turn a user's deny back into a question.
fn split_untrusted(layer: toml::Table) -> (toml::Table, Vec<String>) {
    let mut safe = toml::Table::new();
    let mut left_out = Vec::new();
    for (key, value) in layer {
        match value {
            value if DATA.untrusted_keys.contains(&key) => {
                safe.insert(key, value);
            }
            toml::Value::Array(rules) if key == "permission" => {
                let (denies, others): (Vec<_>, Vec<_>) = rules
                    .into_iter()
                    .partition(|r| r.get("action").and_then(toml::Value::as_str) == Some("deny"));
                left_out.extend(
                    others
                        .iter()
                        .map(|r| format!("[[permission]] {}", rule_line(r))),
                );
                if !denies.is_empty() {
                    safe.insert(key, toml::Value::Array(denies));
                }
            }
            value => left_out.extend(listed(&key, &value)),
        }
    }
    (safe, left_out)
}

/// How a left-out setting is shown: a table of named entries (`[mcp.<name>]`) one entry a line,
/// each value under a `masked` key by name only.
fn listed(key: &str, value: &toml::Value) -> Vec<String> {
    match value {
        toml::Value::Table(entries)
            if !entries.is_empty() && entries.values().all(|v| v.is_table()) =>
        {
            entries
                .iter()
                .map(|(name, entry)| format!("[{key}.{name}] {}", fields(entry)))
                .collect()
        }
        value => vec![format!("{key} = {}", shown(key, value))],
    }
}

fn fields(entry: &toml::Value) -> String {
    let Some(entry) = entry.as_table() else {
        return String::new();
    };
    entry
        .iter()
        .map(|(key, value)| format!("{key} = {}", shown(key, value)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `value` as written, or by its keys only when `key` may hold secrets.
fn shown(key: &str, value: &toml::Value) -> String {
    if !DATA.masked.iter().any(|m| m == key) {
        return value.to_string();
    }
    match value.as_table() {
        Some(table) => {
            let names: Vec<String> = table
                .keys()
                .map(|name| format!("{name} = {}", DATA.mask))
                .collect();
            format!("{{ {} }}", names.join(", "))
        }
        None => DATA.mask.clone(),
    }
}

fn rule_line(rule: &toml::Value) -> String {
    let field = |name: &str| rule.get(name).and_then(toml::Value::as_str).unwrap_or("?");
    format!(
        "{} {} {}",
        field("permission"),
        field("pattern"),
        field("action")
    )
}

fn is_trusted(paths: &Paths) -> anyhow::Result<bool> {
    Ok(load(paths)?.projects.contains(&project(paths)))
}

pub fn trust(paths: &Paths) -> anyhow::Result<()> {
    let mut store = load(paths)?;
    let project = project(paths);
    if !store.projects.contains(&project) {
        store.projects.push(project);
    }
    io::fs::write_json(&paths.trusted(), &store, io::fs::Access::Private)?;
    Ok(())
}

/// What becomes of a project config that asked for more than an untrusted folder gets. With
/// no one to ask, a note on stderr; else the user sees what it would change and is asked, and
/// a yes is remembered. `true` when the folder is now trusted.
pub fn settle(paths: &Paths, untrusted: &Untrusted, can_ask: bool) -> anyhow::Result<bool> {
    if !can_ask {
        io::term::warn(&ignored(untrusted));
        return Ok(false);
    }
    io::term::out(&warning(untrusted));
    let yes = io::term::confirm(&DATA.question, false)?.unwrap_or(false);
    if yes {
        trust(paths)?;
    } else {
        io::term::out(&format!("{}\n", DATA.declined));
    }
    Ok(yes)
}

/// Shown before asking: what the project would change, and why that is dangerous.
fn warning(untrusted: &Untrusted) -> String {
    let file = untrusted.file.display().to_string();
    let mut text = template::fill(&DATA.warning, &[("file", &file)]);
    text.push('\n');
    for key in &untrusted.keys {
        text.push_str(&format!("  {key}\n"));
    }
    text.push_str(&DATA.danger);
    text.push('\n');
    text
}

/// For a run that cannot ask.
fn ignored(untrusted: &Untrusted) -> String {
    let file = untrusted.file.display().to_string();
    template::fill(
        &DATA.ignored,
        &[("keys", &untrusted.keys.join(", ")), ("file", &file)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_parses() {
        LazyLock::force(&DATA);
    }

    #[test]
    fn trusting_one_folder_trusts_only_that_folder() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().join(".crowbot"), "/work/a".into());
        let other = Paths::at(home.path().join(".crowbot"), "/work/b".into());
        assert!(!is_trusted(&paths).unwrap());
        trust(&paths).unwrap();
        trust(&paths).unwrap();
        assert!(is_trusted(&paths).unwrap());
        assert!(!is_trusted(&other).unwrap());
        assert_eq!(load(&paths).unwrap().projects.len(), 1);
    }
}
