use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::text::choices;

static LEVELS: LazyLock<Catalog> = data::load!("effort.toml");

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
    /// A role in data/theme.toml.
    pub color: String,
}

pub fn levels() -> &'static [Level] {
    &LEVELS.level
}

pub fn get(id: &str) -> Option<&'static Level> {
    levels().iter().find(|l| l.id == id)
}

/// `Err` lists the levels there are.
pub fn find(id: &str) -> anyhow::Result<&'static Level> {
    get(id).ok_or_else(|| {
        let levels = levels()
            .iter()
            .map(|l| (l.id.as_str(), Some(l.summary.clone())));
        choices::unknown("effort", id, levels)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_validates() {
        assert!(!levels().is_empty());
        find("high").unwrap();
        assert!(find("ludicrous").is_err());
        for level in levels() {
            assert!(
                crate::text::theme::get().has(&level.color),
                "{} names unknown colour {}",
                level.id,
                level.color
            );
        }
    }
}
