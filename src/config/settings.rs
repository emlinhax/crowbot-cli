use std::path::Path;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

use crate::config::paths::Paths;
use crate::config::trust;
use crate::data;
use crate::permission::lists;
use crate::permission::rule::Rule;
use crate::{effort, io, mode};

const DEFAULTS: &str = data::raw!("defaults.toml");

/// Unknown keys are refused: a misspelled `[[permissions]]` would otherwise drop a deny rule
/// without a word.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub model: String,
    /// `None` leaves effort to the model's own default.
    #[serde(default)]
    pub effort: Option<String>,
    pub mode: String,
    /// Every layer's rules, in layer order; the last match wins.
    #[serde(default)]
    pub permission: Vec<Rule>,
    /// Whether a release build updates itself (see data/defaults.toml).
    pub auto_update: bool,
    /// A shell to run commands in, overriding the search in data/shells.toml.
    #[serde(default)]
    pub shell: Option<String>,
    /// MCP servers to start with the session, by name.
    #[serde(default, deserialize_with = "crate::mcp::config::servers")]
    pub mcp: std::collections::BTreeMap<String, crate::mcp::config::Server>,
    /// Whether crowbot offers the MCP servers the user's other apps list.
    pub mcp_discovery: bool,
    /// What an untrusted project's config asked for and did not get.
    #[serde(skip)]
    pub untrusted: Option<trust::Untrusted>,
    /// What is said once at the start of servers found in other apps' settings, by server.
    #[serde(skip)]
    pub found_notes: Vec<(String, String)>,
}

/// Values from command-line flags: the last and strongest layer. One that is not given leaves
/// the layers before it alone. Declared here once, for the CLI to parse (help: data/cli.toml).
#[derive(Clone, Debug, Default, Serialize, clap::Args)]
pub struct Overrides {
    #[arg(long, global = true)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[arg(long, global = true)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[arg(long, global = true)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

pub fn load(paths: &Paths, flags: &Overrides) -> anyhow::Result<Settings> {
    let mut merged = defaults()?;
    let user = paths.user_config();
    if let Some(layer) = read_layer(&user)? {
        merge(&mut merged, layer);
        check(&merged, &user)?;
    }
    let mut untrusted = None;
    let project = paths.project_config();
    // Started in the home directory, the project config is the user's own file.
    if !io::fs::same_file(&project, &user)
        && let Some(layer) = read_layer(&project)?
    {
        let (layer, left_out) = trust::admit(paths, &project, layer)?;
        untrusted = left_out;
        merge(&mut merged, layer);
        check(&merged, &project)?;
    }
    merge(&mut merged, toml::Table::try_from(flags)?);
    let mut settings: Settings = toml::Value::Table(merged).try_into()?;
    settings.untrusted = untrusted;
    if io::env::var("CROWBOT_NO_UPDATE").is_some() {
        settings.auto_update = false;
    }
    if let Some(level) = &settings.effort {
        effort::find(level)?;
    }
    mode::find(&settings.mode)?;
    Ok(settings)
}

/// Checked after each layer, so an error names the file that caused it.
fn check(merged: &toml::Table, file: &Path) -> anyhow::Result<()> {
    toml::Value::Table(merged.clone())
        .try_into::<Settings>()
        .map(drop)
        .with_context(|| file.display().to_string())
}

fn read_layer(file: &Path) -> anyhow::Result<Option<toml::Table>> {
    let Some(text) = io::fs::read_string(file)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .with_context(|| file.display().to_string())
        .map(Some)
}

/// Later layers win; arrays of tables (`[[permission]]`) append, since a later rule overrides an
/// earlier one.
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

/// The built-in rules alone: a realistic stack for tests, and what a battery run had.
pub fn default_rules() -> Vec<Rule> {
    toml::Value::Table(defaults().unwrap())
        .try_into::<Settings>()
        .unwrap()
        .permission
}

/// data/defaults.toml with each `list = "<name>"` rule spelled out as one rule per pattern
/// (data/command_lists.toml). Only the built-in rules name lists; a user's rules are plain.
fn defaults() -> anyhow::Result<toml::Table> {
    let mut table: toml::Table = toml::from_str(DEFAULTS).context("data/defaults.toml")?;
    if let Some(toml::Value::Array(rules)) = table.get_mut("permission") {
        let mut spelled = Vec::new();
        for rule in rules.drain(..) {
            let Some(name) = rule.get("list").and_then(toml::Value::as_str) else {
                spelled.push(rule);
                continue;
            };
            let patterns = lists::patterns(name)
                .ok_or_else(|| anyhow!("data/defaults.toml names no list {name}"))?;
            for pattern in patterns {
                let mut one = rule.as_table().cloned().unwrap_or_default();
                one.remove("list");
                one.insert("pattern".into(), pattern.into());
                spelled.push(one.into());
            }
        }
        *rules = spelled;
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let settings: Settings = toml::Value::Table(defaults().unwrap()).try_into().unwrap();
        let bundled = crate::models::snapshot();
        assert!(
            bundled.iter().any(|m| m.id == settings.model),
            "{}",
            settings.model
        );
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
    fn an_untrusted_projects_server_is_named_without_its_secrets() {
        let (_root, paths) = project_with(
            "[mcp.github]\ncommand = 'npx'\nargs = ['server']\nenv = { GITHUB_TOKEN = 'sekret' }",
        );
        let settings = load(&paths, &Overrides::default()).unwrap();
        let keys = settings.untrusted.unwrap().keys;
        assert_eq!(keys.len(), 1);
        assert!(keys[0].starts_with("[mcp.github] "), "{keys:?}");
        assert!(keys[0].contains("command = \"npx\""), "{keys:?}");
        assert!(keys[0].contains("GITHUB_TOKEN = …"), "{keys:?}");
        assert!(!keys[0].contains("sekret"), "{keys:?}");
    }

    /// Settings from a user config `user` and a trusted project's `project`.
    fn layered(user: &str, project: &str) -> anyhow::Result<Settings> {
        let (_root, paths) = project_with(project);
        io::fs::write_atomic(
            &paths.user_config(),
            user.as_bytes(),
            io::fs::Access::Shared,
        )
        .unwrap();
        trust::trust(&paths).unwrap();
        load(&paths, &Overrides::default())
    }

    #[test]
    fn mcp_servers_merge_by_name_and_a_later_layer_turns_one_off() {
        let user =
            "[mcp.fake]\ncommand = 'fake-mcp'\nenv = { A = '1' }\n[mcp.other]\ncommand = 'x'";
        let settings = layered(
            user,
            "[mcp.fake]\nenv = { B = '2' }\n[mcp.other]\nenabled = false",
        )
        .unwrap();
        let fake = &settings.mcp["fake"];
        let crate::mcp::config::Reach::Program { command, env, .. } = &fake.reach else {
            panic!("{:?}", fake.reach);
        };
        assert_eq!(command, "fake-mcp");
        assert_eq!(env.len(), 2);
        assert!(!settings.mcp["other"].enabled);
    }

    #[test]
    fn an_mcp_server_needs_a_command_or_a_url_and_a_name_its_tools_can_carry() {
        for (config, says) in [
            ("[mcp.fake]\nargs = ['x']", "needs a `command`"),
            (
                "[mcp.fake]\ncommand = 'x'\nurl = 'http://127.0.0.1:1/mcp'",
                "not both",
            ),
            ("[mcp.'two words']\ncommand = 'x'", "a server's name"),
            ("[mcp.a__b]\ncommand = 'x'", "without `__`"),
        ] {
            let err = format!("{:#}", layered("", config).unwrap_err());
            assert!(
                err.contains(says) && err.contains("config.toml"),
                "{config}: {err}"
            );
        }
    }

    #[test]
    fn an_mcp_server_at_a_url_keeps_its_headers() {
        let settings = layered(
            "[mcp.ida]\nurl = 'http://127.0.0.1:13337/mcp'\nheaders = { Authorization = 'Bearer t' }",
            "",
        )
        .unwrap();
        let crate::mcp::config::Reach::Url { url, headers } = &settings.mcp["ida"].reach else {
            panic!("{:?}", settings.mcp["ida"].reach);
        };
        assert_eq!(url, "http://127.0.0.1:13337/mcp");
        assert_eq!(headers["Authorization"], "Bearer t");
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
    fn a_misspelled_or_mistyped_key_is_refused_naming_its_file() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("repo"));
        let deny = "[[permissions]]\npermission = 'bash'\npattern = 'rm *'\naction = 'deny'";
        io::fs::write_atomic(
            &paths.user_config(),
            deny.as_bytes(),
            io::fs::Access::Shared,
        )
        .unwrap();
        let err = format!("{:#}", load(&paths, &Overrides::default()).unwrap_err());
        assert!(err.contains("permissions"), "{err}");
        assert!(
            err.contains(&paths.user_config().display().to_string()),
            "{err}"
        );

        let (_root, paths) = project_with("model = 5");
        let err = format!("{:#}", load(&paths, &Overrides::default()).unwrap_err());
        assert!(
            err.contains(&paths.project_config().display().to_string()),
            "{err}"
        );
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
