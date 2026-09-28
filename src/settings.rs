use anyhow::Context;
use serde::Deserialize;

use crate::paths::Paths;
use crate::{effort, io};

const DEFAULTS: &str = include_str!("../data/defaults.toml");

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub model: String,
    /// `None` leaves effort to the model's own default.
    #[serde(default)]
    pub effort: Option<String>,
}

/// Values from command-line flags: the last and strongest layer.
#[derive(Debug, Default)]
pub struct Overrides {
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub fn load(paths: &Paths, flags: &Overrides) -> anyhow::Result<Settings> {
    let mut merged: toml::Table = toml::from_str(DEFAULTS).context("data/defaults.toml")?;
    for file in [paths.user_config(), paths.project_config()] {
        if let Some(text) = io::fs::read_string(&file)? {
            let layer: toml::Table =
                toml::from_str(&text).with_context(|| file.display().to_string())?;
            merge(&mut merged, layer);
        }
    }
    let mut settings: Settings = toml::Value::Table(merged).try_into()?;
    if let Some(model) = &flags.model {
        settings.model.clone_from(model);
    }
    if flags.effort.is_some() {
        settings.effort.clone_from(&flags.effort);
    }
    if let Some(level) = &settings.effort {
        effort::validate(level)?;
    }
    Ok(settings)
}

fn merge(base: &mut toml::Table, layer: toml::Table) {
    for (key, value) in layer {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(inner)), toml::Value::Table(over)) => merge(inner, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// The one place the process environment is read; empty variables count as unset.
#[allow(clippy::disallowed_methods)]
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let settings: Settings = toml::from_str(DEFAULTS).unwrap();
        assert!(!settings.model.is_empty());
    }

    #[test]
    fn later_layers_win_and_tables_merge() {
        let mut base: toml::Table = toml::from_str("model = 'a'\n[t]\nx = 1\ny = 2").unwrap();
        merge(
            &mut base,
            toml::from_str("model = 'b'\n[t]\ny = 3").unwrap(),
        );
        assert_eq!(base["model"].as_str(), Some("b"));
        assert_eq!(base["t"]["x"].as_integer(), Some(1));
        assert_eq!(base["t"]["y"].as_integer(), Some(3));
    }

    #[test]
    fn project_config_overrides_user_config_and_flags_override_both() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), project.path().into());
        io::fs::write_atomic(
            &paths.user_config(),
            b"model = 'user'",
            io::fs::Access::Shared,
        )
        .unwrap();
        assert_eq!(load(&paths, &Overrides::default()).unwrap().model, "user");
        io::fs::write_atomic(
            &paths.project_config(),
            b"model = 'project'",
            io::fs::Access::Shared,
        )
        .unwrap();
        assert_eq!(
            load(&paths, &Overrides::default()).unwrap().model,
            "project"
        );
        let flags = Overrides {
            model: Some("flag".into()),
            ..Overrides::default()
        };
        assert_eq!(load(&paths, &flags).unwrap().model, "flag");
    }
}
