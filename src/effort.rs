use std::sync::LazyLock;

use anyhow::bail;
use serde::Deserialize;

const SRC: &str = include_str!("../data/effort.toml");

static LEVELS: LazyLock<Catalog> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/effort.toml is checked by tests"));

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    level: Vec<Level>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub id: String,
    pub summary: String,
}

pub fn levels() -> &'static [Level] {
    &LEVELS.level
}

pub fn validate(id: &str) -> anyhow::Result<()> {
    if levels().iter().any(|l| l.id == id) {
        return Ok(());
    }
    let choices: Vec<String> = levels()
        .iter()
        .map(|l| format!("{} ({})", l.id, l.summary))
        .collect();
    bail!(
        "unknown effort `{id}`; choose one of: {}",
        choices.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_validates() {
        assert!(!levels().is_empty());
        validate("high").unwrap();
        assert!(validate("ludicrous").is_err());
    }
}
