use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::paths::Paths;
use crate::permission::rule::Rule;
use crate::{effort, io, mode};

const DEFAULTS: &str = include_str!("../data/defaults.toml");

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub model: String,
    /// `None` leaves effort to the model's own default.
    #[serde(default)]
    pub effort: Option<String>,
    pub mode: String,
    /// Every layer's rules, in layer order; the last match wins.
    #[serde(default)]
    pub permission: Vec<Rule>,
    /// A shell to run commands in, overriding the search in data/shells.toml.
    #[serde(default)]
    pub shell: Option<String>,
}

/// Values from command-line flags: the last and strongest layer.
#[derive(Debug, Default)]
pub struct Overrides {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
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
    if let Some(mode) = &flags.mode {
        settings.mode.clone_from(mode);
    }
    if let Some(level) = &settings.effort {
        effort::validate(level)?;
    }
    mode::find(&settings.mode).map_err(|e| anyhow!(e))?;
    Ok(settings)
}

/// Later layers win; arrays of tables (`[[permission]]`) append, because their order is their
/// meaning: a later rule overrides an earlier one.
fn merge(base: &mut toml::Table, layer: toml::Table) {
    for (key, value) in layer {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(inner)), toml::Value::Table(over)) => merge(inner, over),
            (Some(toml::Value::Array(inner)), toml::Value::Array(over))
                if over.iter().all(toml::Value::is_table) =>
            {
                inner.extend(over);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// The built-in rules alone, for tests that need a realistic rule stack.
#[cfg(test)]
pub fn default_rules() -> Vec<Rule> {
    toml::from_str::<Settings>(DEFAULTS).unwrap().permission
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
        assert!(mode::get(&settings.mode).is_some());
        assert!(!settings.permission.is_empty());
    }

    #[test]
    fn later_layers_win_tables_merge_and_rule_lists_append() {
        let mut base: toml::Table = toml::from_str(
            "model = 'a'\n[t]\nx = 1\ny = 2\n[[permission]]\npermission='bash'\npattern='*'\naction='ask'",
        )
        .unwrap();
        merge(
            &mut base,
            toml::from_str(
                "model = 'b'\n[t]\ny = 3\n[[permission]]\npermission='bash'\npattern='ls'\naction='allow'",
            )
            .unwrap(),
        );
        assert_eq!(base["model"].as_str(), Some("b"));
        assert_eq!(base["t"]["x"].as_integer(), Some(1));
        assert_eq!(base["t"]["y"].as_integer(), Some(3));
        assert_eq!(base["permission"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn project_config_overrides_user_config_and_flags_override_both() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), project.path().into());
        let write = |path: &std::path::Path, text: &str| {
            io::fs::write_atomic(path, text.as_bytes(), io::fs::Access::Shared).unwrap();
        };
        write(&paths.user_config(), "model = 'user'");
        assert_eq!(load(&paths, &Overrides::default()).unwrap().model, "user");
        write(&paths.project_config(), "model = 'project'");
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

    #[test]
    fn unknown_modes_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        let flags = Overrides {
            mode: Some("yolo".into()),
            ..Overrides::default()
        };
        let err = load(&paths, &flags).unwrap_err().to_string();
        assert!(err.contains("unknown mode"), "{err}");
    }
}
