//! MCP servers the user's other apps already list (data/mcp_sources.toml): read from each app's
//! own file in the shape that app writes, made crowbot's, one entry per server.
// CEILING: Claude Code's plugins (~/.claude/plugins, `${CLAUDE_PLUGIN_ROOT}`), Claude Desktop's
// extensions and servers over the legacy SSE transport are not read. Upgrade: a source or a
// shape each.

pub mod choose;
mod common;
mod opencode;
mod vars;

use std::path::Path;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::data;
use crate::io;
use crate::mcp::config::{self, Reach};
use crate::text::template::{fill, fill_with};
use crate::text::{digest, jsonc};

static SOURCES: LazyLock<Sources> = data::load!("mcp_sources.toml");
static TEXT: LazyLock<Text> = data::load!("mcp_discover.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    source: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    scope: Scope,
    paths: Vec<String>,
    format: Format,
    at: Vec<String>,
    shape: Shape,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    found: String,
    found_project: String,
    danger: String,
    entry: String,
    secrets: String,
    question: String,
    question_project: String,
    unchosen: String,
    left_out: String,
}

/// Whose file lists a server: the user's own apps, or the project, which is asked about apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Project,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Format {
    Json,
    Jsonc,
    Toml,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Shape {
    Common,
    Opencode,
}

/// One server other apps list, as crowbot would reach it.
#[derive(Clone, Debug)]
pub struct Found {
    /// The name crowbot gives it, made to fit.
    pub name: String,
    pub reach: Reach,
    pub scope: Scope,
    /// The apps or files listing it, as shown, in the order read.
    pub sources: Vec<String>,
    /// What it runs or reaches (not the values of its env or headers), so a choice about it holds
    /// until that changes.
    pub fingerprint: String,
}

/// Every server the sources list for `project`, with `var` naming the environment, and what is
/// said of those left out, by name. A server several apps list is one entry; a name two
/// different servers share stays with the first source to list it.
pub fn found(
    project: &Path,
    var: &dyn Fn(&str) -> Option<String>,
) -> (Vec<Found>, Vec<(String, String)>) {
    let project_key = project.display().to_string();
    let mut found: Vec<Found> = Vec::new();
    let mut notes = Vec::new();
    for source in &SOURCES.source {
        let at: Vec<String> = source
            .at
            .iter()
            .map(|key| fill(key, &[("project", &project_key)]))
            .collect();
        for path in &source.paths {
            let Some(document) = read(&filled_path(path, &project_key, var), &source.format) else {
                continue;
            };
            let map = at.iter().try_fold(&document, |v, key| v.get(key));
            let Some(Value::Object(map)) = map else {
                continue;
            };
            for (listed, entry) in map {
                let read = match source.shape {
                    Shape::Common => common::reach(entry),
                    Shape::Opencode => opencode::reach(entry),
                };
                let (Some(name), Some(reach)) = (
                    config::fit(listed),
                    read.and_then(|reach| vars::filled(reach, var)),
                ) else {
                    continue;
                };
                let fingerprint = fingerprint(&reach);
                match found.iter_mut().find(|f| f.name == name) {
                    Some(known) if known.fingerprint == fingerprint => {
                        if !known.sources.contains(&source.name) {
                            known.sources.push(source.name.clone());
                        }
                    }
                    Some(known) => {
                        let vars = [
                            ("name", name.as_str()),
                            ("source", &source.name),
                            ("first", &known.sources[0]),
                        ];
                        notes.push((name.clone(), fill(&TEXT.left_out, &vars)));
                    }
                    None => found.push(Found {
                        name,
                        reach,
                        scope: source.scope,
                        sources: vec![source.name.clone()],
                        fingerprint,
                    }),
                }
            }
        }
    }
    (found, notes)
}

/// `path` with `{project}` and each `{NAME}` filled; empty when it names an unset variable.
fn filled_path(path: &str, project: &str, var: &dyn Fn(&str) -> Option<String>) -> String {
    let path = fill(path, &[("project", project)]);
    let path = fill_with(&path, |name| var(name).map(std::borrow::Cow::Owned));
    if path.contains('{') {
        String::new()
    } else {
        path
    }
}

/// The document at `path` as JSON, or `None` when it is missing or not what its app writes.
fn read(path: &str, format: &Format) -> Option<Value> {
    if path.is_empty() {
        return None;
    }
    let text = io::fs::read_string(Path::new(path)).ok()??;
    match format {
        Format::Json => serde_json::from_str(&text).ok(),
        Format::Jsonc => serde_json::from_str(&jsonc::plain(&text)).ok(),
        Format::Toml => serde_json::to_value(toml::from_str::<toml::Table>(&text).ok()?).ok(),
    }
}

fn fingerprint(reach: &Reach) -> String {
    let what = match reach {
        Reach::Program { command, args, env } => {
            json!({"command": command, "args": args, "env": env.keys().collect::<Vec<_>>()})
        }
        Reach::Url { url, headers } => {
            json!({"url": url, "headers": headers.keys().collect::<Vec<_>>()})
        }
    };
    digest::sha256_hex(what.to_string().as_bytes())[..32].to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    /// A user's folder and a project, with the variables a machine would set pointing into it.
    struct Machine {
        dir: tempfile::TempDir,
        project: PathBuf,
    }

    impl Machine {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let project = dir.path().join("proj");
            Self { dir, project }
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.dir.path().join(rel);
            io::fs::make_dir(path.parent().unwrap()).unwrap();
            io::fs::write_atomic(&path, text.as_bytes(), io::fs::Access::Shared).unwrap();
        }

        fn found(&self) -> (Vec<Found>, Vec<(String, String)>) {
            let home = self.dir.path().join("home");
            let var = |name: &str| match name {
                "HOME" => Some(home.display().to_string()),
                "XDG_CONFIG_HOME" => Some(home.join(".config").display().to_string()),
                "TOKEN" => Some("t0k".to_owned()),
                _ => None,
            };
            found(&self.project, &var)
        }
    }

    fn program(command: &str, args: &[&str]) -> Reach {
        Reach::Program {
            command: command.into(),
            args: args.iter().map(|a| (*a).into()).collect(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn each_apps_shape_is_read_and_a_server_several_apps_list_is_one() {
        let m = Machine::new();
        let ida =
            r#""github.com/mrexodia/ida-pro-mcp": {"command": "python", "args": ["server.py"]}"#;
        m.write(
            "home/.config/Claude/claude_desktop_config.json",
            &format!(r#"{{"mcpServers": {{{ida}}}}}"#),
        );
        m.write(
            "home/.cursor/mcp.json",
            &format!(
                r#"{{"mcpServers": {{{ida}, "off": {{"command": "x", "disabled": true}},
                  "old": {{"type": "sse", "url": "http://h/sse"}}}}}}"#
            ),
        );
        m.write(
            "home/.config/Code/User/mcp.json",
            r#"{
              // VS Code's own
              "servers": {"web": {"type": "http", "url": "http://127.0.0.1:13337/mcp",
                                  "headers": {"Authorization": "Bearer ${TOKEN}"}},},
            }"#,
        );
        m.write(
            "home/.codex/config.toml",
            "[mcp_servers.docs]\ncommand = 'npx'\nargs = ['-y', 'docs']\n\
             [mcp_servers.gone]\ncommand = 'x'\nenabled = false\n",
        );
        m.write(
            "home/.config/opencode/opencode.json",
            r#"{"mcp": {"oc": {"type": "local", "command": ["bun", "x.ts"], "environment": {"A": "1"}}}}"#,
        );
        let (found, notes) = m.found();
        assert!(notes.is_empty(), "{notes:?}");
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["ida-pro-mcp", "web", "docs", "oc"]);
        assert_eq!(found[0].sources, ["Claude Desktop", "Cursor"]);
        assert_eq!(found[0].reach, program("python", &["server.py"]));
        let Reach::Url { url, headers } = &found[1].reach else {
            panic!("{:?}", found[1].reach);
        };
        assert_eq!(url, "http://127.0.0.1:13337/mcp");
        assert_eq!(headers["Authorization"], "Bearer t0k");
        assert_eq!(found[2].reach, program("npx", &["-y", "docs"]));
        let Reach::Program { command, args, env } = &found[3].reach else {
            panic!("{:?}", found[3].reach);
        };
        assert_eq!(
            (command.as_str(), args.as_slice()),
            ("bun", &["x.ts".to_owned()][..])
        );
        assert_eq!(env["A"], "1");
        assert!(found.iter().all(|f| f.scope == Scope::User));
    }

    #[test]
    fn this_projects_entries_and_the_files_a_repository_ships_are_read() {
        let m = Machine::new();
        let key = m.project.display().to_string();
        let claude =
            json!({"projects": {key: {"mcpServers": {"mine": {"command": "local-tool"}}}}});
        m.write("home/.claude.json", &claude.to_string());
        m.write(
            "proj/.mcp.json",
            r#"{"mcpServers": {"repo": {"command": "./run.sh"}}}"#,
        );
        let (found, _) = m.found();
        let by = |name: &str| found.iter().find(|f| f.name == name).unwrap();
        assert_eq!(
            (by("mine").scope, by("mine").sources[0].as_str()),
            (Scope::User, "Claude Code")
        );
        assert_eq!(
            (by("repo").scope, by("repo").sources[0].as_str()),
            (Scope::Project, ".mcp.json")
        );
    }

    #[test]
    fn a_name_two_different_servers_share_stays_with_the_first_source() {
        let m = Machine::new();
        let listing =
            |command: &str| format!(r#"{{"mcpServers": {{"x": {{"command": "{command}"}}}}}}"#);
        m.write(
            "home/.config/Claude/claude_desktop_config.json",
            &listing("a"),
        );
        m.write("home/.cursor/mcp.json", &listing("b"));
        let (found, notes) = m.found();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reach, program("a", &[]));
        assert_eq!(notes.len(), 1);
        let note = &notes[0].1;
        assert!(
            note.contains("Cursor") && note.contains("Claude Desktop"),
            "{note}"
        );
    }

    #[test]
    fn a_changed_command_changes_the_fingerprint_a_rotated_secret_does_not() {
        let with = |command: &str, token: &str| Reach::Program {
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::from([("TOKEN".to_owned(), token.to_owned())]),
        };
        assert_eq!(fingerprint(&with("a", "1")), fingerprint(&with("a", "2")));
        assert_ne!(fingerprint(&with("a", "1")), fingerprint(&with("b", "1")));
    }
}
