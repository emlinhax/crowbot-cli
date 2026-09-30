//! Project folders whose own .crowbot/config.toml may loosen crowbot's settings. The list lives in
//! crowbot's home, where a repository cannot write itself into it.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::io;
use crate::paths::Paths;
use crate::text::template;

const SRC: &str = include_str!("../data/trust.toml");

static TEXT: LazyLock<Text> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/trust.toml is checked by tests"));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
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

fn load(paths: &Paths) -> anyhow::Result<Store> {
    let file = paths.trusted();
    match io::fs::read_string(&file)? {
        Some(text) => serde_json::from_str(&text).with_context(|| file.display().to_string()),
        None => Ok(Store::default()),
    }
}

fn project(paths: &Paths) -> String {
    paths.project.display().to_string()
}

pub fn is_trusted(paths: &Paths) -> anyhow::Result<bool> {
    Ok(load(paths)?.projects.contains(&project(paths)))
}

pub fn trust(paths: &Paths) -> anyhow::Result<()> {
    let mut store = load(paths)?;
    let project = project(paths);
    if !store.projects.contains(&project) {
        store.projects.push(project);
    }
    let file = paths.trusted();
    let json = serde_json::to_string_pretty(&store)?;
    io::fs::write_atomic(&file, json.as_bytes(), io::fs::Access::Private)
        .with_context(|| file.display().to_string())
}

/// Shown before asking: what the project would change, and why that is dangerous.
pub fn warning(file: &Path, keys: &[String]) -> String {
    let file = file.display().to_string();
    let mut text = template::fill(&TEXT.warning, &[("file", &file)]);
    text.push('\n');
    for key in keys {
        text.push_str(&format!("  {key}\n"));
    }
    text.push_str(&TEXT.danger);
    text.push('\n');
    text
}

pub fn question() -> &'static str {
    &TEXT.question
}

pub fn declined() -> &'static str {
    &TEXT.declined
}

/// For a run that cannot ask.
pub fn ignored(file: &Path, keys: &[String]) -> String {
    let file = file.display().to_string();
    template::fill(
        &TEXT.ignored,
        &[("keys", &keys.join(", ")), ("file", &file)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_parses() {
        LazyLock::force(&TEXT);
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
