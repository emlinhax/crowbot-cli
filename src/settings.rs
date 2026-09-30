use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::paths::Paths;
use crate::permission::rule::Rule;
use crate::{effort, io, mode, trust};

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
    /// What an untrusted project's config asked for and did not get.
    #[serde(skip)]
    pub untrusted: Option<Untrusted>,
}

#[derive(Debug)]
pub struct Untrusted {
    pub file: PathBuf,
    /// Each setting left out, as written: `mode = "auto"`.
    pub keys: Vec<String>,
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
    let user = paths.user_config();
    if let Some(layer) = read_layer(&user)? {
        merge(&mut merged, layer);
    }
    let mut untrusted = None;
    let project = paths.project_config();
    // Started in the home directory, the project config is the user's own file.
    if !io::fs::same_file(&project, &user)
        && let Some(layer) = read_layer(&project)?
    {
        let layer = if trust::is_trusted(paths)? {
            layer
        } else {
            let (safe, keys) = split_untrusted(layer);
            if !keys.is_empty() {
                untrusted = Some(Untrusted {
                    file: project,
                    keys,
                });
            }
            safe
        };
        merge(&mut merged, layer);
    }
    let mut settings: Settings = toml::Value::Table(merged).try_into()?;
    settings.untrusted = untrusted;
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

fn read_layer(file: &Path) -> anyhow::Result<Option<toml::Table>> {
    let Some(text) = io::fs::read_string(file)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .with_context(|| file.display().to_string())
        .map(Some)
}

/// What a project may set without being trusted: what it runs on, and deny rules. Under
/// last-match-wins an appended deny can only tighten; an `ask` could turn a user's deny back
/// into a question, and anything else (`mode`, `shell`) can run commands unasked.
const UNTRUSTED_KEYS: [&str; 2] = ["model", "effort"];

fn split_untrusted(layer: toml::Table) -> (toml::Table, Vec<String>) {
    let mut safe = toml::Table::new();
    let mut left_out = Vec::new();
    for (key, value) in layer {
        match value {
            value if UNTRUSTED_KEYS.contains(&key.as_str()) => {
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
            value => left_out.push(format!("{key} = {value}")),
        }
    }
    (safe, left_out)
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

    fn project_with(config: &str) -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("repo"));
        io::fs::write_atomic(
            &paths.project_config(),
            config.as_bytes(),
            io::fs::Access::Shared,
        )
        .unwrap();
        (root, paths)
    }

    const ALLOW_ALL: &str = "[[permission]]\npermission = '*'\npattern = '*'\naction = 'allow'";

    #[test]
    fn an_untrusted_project_cannot_loosen_anything() {
        for (config, left_out) in [
            ("mode = 'auto'", "mode = \"auto\""),
            (ALLOW_ALL, "[[permission]] * * allow"),
            ("shell = '.crowbot/sh'", "shell = \".crowbot/sh\""),
        ] {
            let (_root, paths) = project_with(config);
            let settings = load(&paths, &Overrides::default()).unwrap();
            assert_eq!(settings.mode, "manual", "{config}");
            assert!(settings.shell.is_none(), "{config}");
            assert!(
                !settings.permission.iter().any(|r| r.pattern == "*"
                    && r.permission == "*"
                    && r.action == crate::permission::rule::Action::Allow),
                "{config}"
            );
            let untrusted = settings.untrusted.expect(config);
            assert_eq!(untrusted.file, paths.project_config());
            assert_eq!(untrusted.keys, vec![left_out.to_owned()]);
        }
    }

    #[test]
    fn an_untrusted_project_may_pick_its_model_and_add_denies() {
        let (_root, paths) = project_with(
            "model = 'project'\n[[permission]]\npermission = 'bash'\npattern = 'rm *'\naction = 'deny'",
        );
        let settings = load(&paths, &Overrides::default()).unwrap();
        assert_eq!(settings.model, "project");
        assert!(settings.permission.iter().any(|r| r.pattern == "rm *"));
        assert!(settings.untrusted.is_none());
    }

    #[test]
    fn a_trusted_project_loads_whole() {
        let (_root, paths) = project_with(&format!("mode = 'auto'\n{ALLOW_ALL}"));
        trust::trust(&paths).unwrap();
        let settings = load(&paths, &Overrides::default()).unwrap();
        assert_eq!(settings.mode, "auto");
        assert!(settings.untrusted.is_none());
    }

    #[test]
    fn the_home_directory_config_is_read_once_as_the_users() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join(".crowbot"), root.path().into());
        io::fs::write_atomic(
            &paths.user_config(),
            b"mode = 'auto'",
            io::fs::Access::Shared,
        )
        .unwrap();
        let settings = load(&paths, &Overrides::default()).unwrap();
        assert_eq!(settings.mode, "auto");
        assert!(settings.untrusted.is_none());
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
