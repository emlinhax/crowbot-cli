//! The kinds of subagent the `task` tool starts: one TOML file per built-in kind in data/agents/,
//! registered in `SOURCES`, then the user's agent files (agent_file.rs): ~/.crowbot/agents/, then
//! a trusted project's .crowbot/agents/, a later kind of the same name replacing an earlier one.
//! Each says what it is for, which of the parent's tools it gets, and the prompt it runs under.
//! Every kind runs under the parent's mode.
// CEILING: a kind cannot be stricter than the mode the user chose. Upgrade: a `mode = "<id>"` key
// giving its subagent a mode cell of its own.
// CEILING: the kinds are process-wide, set by `init` before the settings and the session load; a
// file changed mid-session counts from the next start. Upgrade: kinds carried on `App`.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

use crate::agent::agent_file;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::config::trust;
use crate::data;
use crate::io;
use crate::tools::Registry;

const SOURCES: &[&str] = &[
    data::raw!("agents/explore.toml"),
    data::raw!("agents/general.toml"),
];

static BUILTIN: LazyLock<Vec<Kind>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|src| data::parse(src, "agents/*.toml"))
        .collect()
});

/// The built-in kinds with the user's agent files over them, once `init` ran.
static LOADED: RwLock<Option<&'static [Kind]>> = RwLock::new(None);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kind {
    pub id: String,
    pub summary: String,
    /// When to pick it, beside `summary`: the jobs it is for.
    #[serde(default)]
    pub when: Option<String>,
    /// The parent's tools it gets, by name; `*` is all of them.
    pub(crate) tools: Vec<String>,
    /// Tools a `*` leaves out.
    #[serde(default)]
    pub(crate) except: Vec<String>,
    pub prompt: String,
    /// The model it runs on unless the user's settings or the call name another.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// What its prompt does to the session's.
    #[serde(default)]
    pub base: Base,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    /// Follows the session's prompt, which a vendor can then cache across kinds.
    #[default]
    Append,
    /// Stands alone, with only the environment block (where, when, which model) kept.
    Replace,
}

pub fn all() -> &'static [Kind] {
    LOADED
        .read()
        .ok()
        .and_then(|loaded| *loaded)
        .unwrap_or(&BUILTIN)
}

pub fn get(id: &str) -> Option<&'static Kind> {
    all().iter().find(|k| k.id == id)
}

/// Reads the user's agent files over the built-in kinds and makes them the kinds from here on.
/// What was wrong with a file, or left out, comes back to be said once.
pub fn init(paths: &Paths) -> Vec<String> {
    let trusted = trust::is_trusted(paths).unwrap_or(false);
    let (kinds, notes) = load(paths, trusted);
    if let Ok(mut loaded) = LOADED.write() {
        // Leaked: a start sets the kinds once, or twice when the folder is trusted on the way.
        *loaded = Some(Box::leak(kinds.into_boxed_slice()));
    }
    notes
}

/// The built-in kinds, then each agent file over them: the user's, then the project's when
/// `trusted`. A file that does not read is left out with a note.
pub fn load(paths: &Paths, trusted: bool) -> (Vec<Kind>, Vec<String>) {
    let mut kinds = BUILTIN.clone();
    let mut notes = Vec::new();
    let registry = Registry::builtin(None);
    let tools: Vec<&str> = registry.each_name().collect();
    let project = paths.project_agents();
    let mut dirs = vec![paths.user_agents()];
    // Started in the home directory, the project's agents are the user's own. An untrusted
    // project's are left for the trust prompt to name (`left_out`).
    if project != dirs[0] && trusted {
        dirs.push(project);
    }
    for dir in dirs {
        for file in files(&dir) {
            let shown = file.display().to_string();
            let text = match io::fs::read_string(&file) {
                Ok(Some(text)) => text,
                Ok(None) => continue,
                Err(e) => {
                    notes.push(format!("{shown}: {e}"));
                    continue;
                }
            };
            match agent_file::parse(&text, &shown, &tools) {
                Ok((kind, said)) => {
                    notes.extend(said);
                    match kinds.iter_mut().find(|k| k.id == kind.id) {
                        Some(earlier) => *earlier = kind,
                        None => kinds.push(kind),
                    }
                }
                Err(why) => notes.push(why),
            }
        }
    }
    (kinds, notes)
}

/// The project's agent files an untrusted folder leaves out, for the trust prompt to name.
pub fn left_out(paths: &Paths) -> Vec<PathBuf> {
    let project = paths.project_agents();
    match project == paths.user_agents() || trust::is_trusted(paths).unwrap_or(false) {
        true => Vec::new(),
        false => files(&project),
    }
}

/// The agent files in `dir`, by name; none when it is missing.
fn files(dir: &Path) -> Vec<PathBuf> {
    let extension = agent_file::extension();
    io::fs::list_dir(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            !e.is_dir
                && Path::new(&e.name)
                    .extension()
                    .is_some_and(|x| x == extension)
        })
        .map(|e| dir.join(e.name))
        .collect()
}

/// The tools a subagent of `kind` gets from its parent's `registry`, `depth` runs below the
/// frontend's: never `task` at the depth limit, so it is never offered a call it could not make.
pub fn tools(kind: &Kind, registry: &Registry, depth: usize) -> Registry {
    let deepest = depth >= limits::get().subagent.max_depth.value;
    let named: Vec<&str> = match kind.tools.iter().any(|t| t == "*") {
        true => registry.each_name().collect(),
        false => kind.tools.iter().map(String::as_str).collect(),
    };
    let names: Vec<&str> = named
        .into_iter()
        .filter(|name| !kind.except.iter().any(|e| e == name))
        .filter(|name| !(deepest && *name == crate::tools::TASK))
        .collect();
    registry.subset(&names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::catalog;
    use crate::tools::testing::{Project, sample};

    fn agent(dir: &Path, name: &str, text: &str) {
        io::fs::write_atomic(&dir.join(name), text.as_bytes(), io::fs::Access::Shared).unwrap();
    }

    #[test]
    fn agent_files_add_kinds_and_replace_a_built_in_the_project_only_when_trusted() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        let mine =
            "---\nname: explore\ndescription: My explore.\ntools: Read\nmodel: kimi-k3\n---\nMine.";
        agent(&paths.user_agents(), "explore.md", mine);
        agent(&paths.user_agents(), "broken.md", "no header here");
        agent(&paths.user_agents(), "notes.txt", "not an agent");
        let reviewer = "---\nname: reviewer\ndescription: Reviews.\n---\nReview.";
        agent(&paths.project_agents(), "reviewer.md", reviewer);

        let (kinds, notes) = load(&paths, false);
        let ids: Vec<&str> = kinds.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(ids, ["explore", "general"]);
        assert_eq!(
            left_out(&paths),
            [paths.project_agents().join("reviewer.md")]
        );
        let explore = &kinds[0];
        assert_eq!(
            (explore.summary.as_str(), explore.prompt.as_str()),
            ("My explore.", "Mine.")
        );
        assert_eq!(explore.tools, ["read"]);
        assert_eq!(explore.model.as_deref(), Some("kimi-k3"));
        assert!(
            notes
                .iter()
                .any(|n| n.contains("broken.md") && n.contains("header")),
            "{notes:?}"
        );

        let (kinds, notes) = load(&paths, true);
        let ids: Vec<&str> = kinds.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(ids, ["explore", "general", "reviewer"]);
        assert_eq!(kinds[2].tools, ["*"]);
        assert!(notes.iter().all(|n| n.contains("broken.md")), "{notes:?}");
        trust::trust(&paths).unwrap();
        assert!(left_out(&paths).is_empty());
    }

    #[test]
    fn kinds_have_unique_ids_and_name_real_tools() {
        let registry = Registry::builtin(None);
        let mut ids: Vec<&str> = all().iter().map(|k| k.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all().len());
        for kind in all() {
            for tool in kind.tools.iter().chain(&kind.except).filter(|t| *t != "*") {
                assert!(registry.get(tool).is_some(), "{}: no tool {tool}", kind.id);
            }
        }
    }

    #[test]
    fn general_gets_every_tool_but_handing_over_the_plan_and_starting_another() {
        let registry = Registry::builtin(None);
        let general = tools(get("general").unwrap(), &registry, 1);
        let want: Vec<&str> = registry
            .each_name()
            .filter(|n| !["plan_exit", crate::tools::TASK].contains(n))
            .collect();
        assert_eq!(general.each_name().collect::<Vec<_>>(), want);
    }

    #[test]
    fn explore_changes_nothing_and_never_gets_task() {
        let explore = get("explore").unwrap();
        let tools = tools(explore, &Registry::builtin(None), 1);
        assert!(tools.get(crate::tools::TASK).is_none());
        let project = Project::new();
        for name in &explore.tools {
            let tool = tools.get(name).unwrap();
            let args = sample(&tool.spec().parameters);
            if let Ok(check) = project.check(tool.as_ref(), &args) {
                for ask in check.asks {
                    assert!(!catalog::changes_things(&ask.permission), "{name}: {ask:?}");
                }
            }
        }
    }
}
