//! The roles the `task` tool can start. Three are built in — one TOML file each in data/agents/,
//! registered in `SOURCES` — and the user adds or overrides one by dropping a file of the same
//! shape in ~/.crowbot/agents/, named for its `id`. Each says what it is for, which of the
//! parent's tools it gets, what it adds to the system prompt, and optionally the model and effort
//! it runs on instead of the session's. Every role runs under the parent's mode.
// CEILING: a role cannot be stricter than the mode the user chose. Upgrade: a `mode = "<id>"` key
// giving its subagent a mode cell of its own.

use std::path::PathBuf;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::models::Model;
use crate::config::paths::Paths;
use crate::config::{limits, prefs};
use crate::data;
use crate::io;
use crate::models::Catalog;
use crate::text::template;
use crate::tools::Registry;

const SOURCES: &[&str] = &[
    data::raw!("agents/reader.toml"),
    data::raw!("agents/worker.toml"),
    data::raw!("agents/researcher.toml"),
];

/// Everything data/prompts/subagents.toml says: the section the system prompt carries, and the
/// note a role's subagent is given when it runs on a model of its own.
static TEXT: LazyLock<Text> = data::load!("prompts/subagents.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    frame: String,
    role: String,
    model: String,
    effort: String,
    own: String,
    own_effort: String,
}

static BUILTIN: LazyLock<Vec<Role>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|src| data::parse(src, "agents/*.toml"))
        .collect()
});

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub id: String,
    pub summary: String,
    /// The parent's tools it gets, by name; `*` is all of them.
    tools: Vec<String>,
    /// Tools a `*` leaves out.
    #[serde(default)]
    except: Vec<String>,
    pub prompt: String,
    /// The model it runs on instead of the session's: a cheap one for reading, a strong one for
    /// work that has to land.
    #[serde(default)]
    pub model: Option<String>,
    /// Its reasoning effort, for a model that reasons.
    #[serde(default)]
    pub effort: Option<String>,
}

impl Role {
    /// What it runs on: the user's choice for this role (`/models`), else what its file pins,
    /// else the session's. A pinned model crowbot no longer serves gives way to the session's, as
    /// a resumed session's does, so a stale pin never blocks a dispatch.
    pub fn binding(
        &self,
        catalog: &Catalog,
        paths: &Paths,
        session: (&Model, Option<&str>),
    ) -> (Model, Option<String>) {
        let chosen = prefs::load(paths).roles.get(&self.id).cloned();
        let model = chosen
            .as_ref()
            .and_then(|c| c.model.clone())
            .or_else(|| self.model.clone())
            .and_then(|id| catalog.get(&id).cloned())
            .unwrap_or_else(|| session.0.clone());
        let effort = chosen
            .and_then(|c| c.effort)
            .or_else(|| self.effort.clone())
            .or_else(|| session.1.map(str::to_owned));
        let effort = model.effort(effort.as_deref()).map(str::to_owned);
        (model, effort)
    }

    /// The model id it runs on, where that is not the session's: what `/models` shows of a role.
    pub fn own_model(&self, paths: &Paths, session_model: &str) -> Option<String> {
        prefs::load(paths)
            .roles
            .get(&self.id)
            .and_then(|c| c.model.clone())
            .or_else(|| self.model.clone())
            .filter(|id| id != session_model)
    }

    /// The effort it asks for, where that is not the session's.
    pub fn own_effort(&self, paths: &Paths, session_effort: Option<&str>) -> Option<String> {
        prefs::load(paths)
            .roles
            .get(&self.id)
            .and_then(|c| c.effort.clone())
            .or_else(|| self.effort.clone())
            .filter(|id| Some(id.as_str()) != session_effort)
    }
}

/// The built-in roles, in their order.
pub fn builtin() -> &'static [Role] {
    &BUILTIN
}

/// Every role this machine has: the built-in ones, then each file in ~/.crowbot/agents/, which
/// overrides the built-in of its id and may add one of its own.
pub fn load(paths: &Paths) -> anyhow::Result<Vec<Role>> {
    let mut roles = builtin().to_vec();
    for (id, file, text) in user_files(paths)? {
        let role: Role =
            toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        if role.id != id {
            anyhow::bail!(
                "{}: declares id `{}`; a role's file is named for its id",
                file.display(),
                role.id
            );
        }
        match roles.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = role,
            None => roles.push(role),
        }
    }
    Ok(roles)
}

/// What a subagent running on a model or effort of its own is told, since its system prompt is the
/// session's.
pub fn said(model: &Model, effort: Option<&str>) -> String {
    let effort = effort.map_or_else(String::new, |level| {
        template::fill(&TEXT.own_effort, &[("effort", level)])
    });
    template::fill(&TEXT.own, &[("model", &model.id), ("effort", &effort)])
        .trim()
        .to_owned()
}

/// The system prompt's section naming every role, with the model each runs on where that is not
/// the session's, so the agent picks a role by its cost as well as its job.
pub fn section(paths: &Paths, model: &str, effort: Option<&str>) -> Option<String> {
    // An unreadable role file is reported where the roles are listed; the prompt goes without.
    let roles = load(paths).ok()?;
    if roles.is_empty() {
        return None;
    }
    let lines: Vec<String> = roles
        .iter()
        .map(|role| {
            let own_model = role.own_model(paths, model).map_or_else(String::new, |id| {
                template::fill(&TEXT.model, &[("model", &id)])
            });
            let own_effort = role
                .own_effort(paths, effort)
                .map_or_else(String::new, |level| {
                    template::fill(&TEXT.effort, &[("effort", &level)])
                });
            template::fill(
                &TEXT.role,
                &[
                    ("id", &role.id),
                    ("summary", &role.summary),
                    ("model", &own_model),
                    ("effort", &own_effort),
                ],
            )
        })
        .collect();
    Some(
        template::fill(&TEXT.frame, &[("roles", &lines.join("\n"))])
            .trim()
            .to_owned(),
    )
}

/// A role by id, in a loaded list.
pub fn find<'a>(roles: &'a [Role], id: &str) -> Option<&'a Role> {
    roles.iter().find(|r| r.id == id)
}

/// Each role file under the user's own folder, by id with its path and text. A file that will not
/// read is skipped: one broken role must not take `/models` and dispatch with it.
fn user_files(paths: &Paths) -> anyhow::Result<Vec<(String, PathBuf, String)>> {
    let dir = paths.roles_dir();
    if !io::fs::is_dir(&dir) {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in io::fs::list_dir(&dir)? {
        let path = dir.join(&entry.name);
        if entry.is_dir || path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let Some(text) = io::fs::read_string(&path)? else {
            continue;
        };
        files.push((id, path, text));
    }
    Ok(files)
}

/// Writes a new role's file into the user's folder, from the built-in template; `Err` when the
/// name is not one a file can hold or the role already exists. Returns the file written.
pub fn write_new(paths: &Paths, id: &str) -> anyhow::Result<PathBuf> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        anyhow::bail!("a role's name is letters, digits, `-` and `_`");
    }
    let dir = paths.roles_dir();
    io::fs::make_dir(&dir)?;
    let file = dir.join(format!("{id}.toml"));
    if io::fs::exists(&file) {
        anyhow::bail!("there is already a role called `{id}`");
    }
    let text = template::fill(data::raw!("agents/_template.toml"), &[("id", id)]);
    io::fs::write_atomic(&file, text.as_bytes(), io::fs::Access::Private)?;
    Ok(file)
}

/// The tools a subagent of `role` gets from its parent's `registry`, `depth` runs below the
/// frontend's: never `task` at the depth limit, so it is never offered a call it could not make.
pub fn tools(role: &Role, registry: &Registry, depth: usize) -> Registry {
    let deepest = depth >= limits::get().subagent.max_depth.value;
    let named: Vec<&str> = match role.tools.iter().any(|t| t == "*") {
        true => registry.each_name().collect(),
        false => role.tools.iter().map(String::as_str).collect(),
    };
    let names: Vec<&str> = named
        .into_iter()
        .filter(|name| !role.except.iter().any(|e| e == name))
        .filter(|name| !(deepest && *name == crate::tools::TASK))
        .collect();
    registry.subset(&names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::catalog as permissions;
    use crate::tools::testing::{Project, sample};

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        (dir, paths)
    }

    fn model(id: &str) -> Model {
        Model {
            id: id.into(),
            ..crate::testing::model(true)
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            models: vec![model("session"), model("pinned"), model("chosen")],
            source: crate::models::Source::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    #[test]
    fn roles_have_unique_ids_and_name_real_tools() {
        let registry = Registry::builtin(None);
        let mut ids: Vec<&str> = builtin().iter().map(|k| k.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), builtin().len());
        for role in builtin() {
            for tool in role.tools.iter().chain(&role.except).filter(|t| *t != "*") {
                assert!(registry.get(tool).is_some(), "{}: no tool {tool}", role.id);
            }
        }
    }

    #[test]
    fn the_worker_gets_every_tool_but_handing_over_the_plan_and_starting_another() {
        let registry = Registry::builtin(None);
        let worker = find(builtin(), "worker").unwrap();
        let tools = tools(worker, &registry, 1);
        let want: Vec<&str> = registry
            .each_name()
            .filter(|n| !["plan_exit", crate::tools::TASK].contains(n))
            .collect();
        assert_eq!(tools.each_name().collect::<Vec<_>>(), want);
    }

    #[test]
    fn roles_that_name_their_tools_change_nothing_and_never_get_task() {
        let project = Project::new();
        for role in builtin()
            .iter()
            .filter(|r| !r.tools.contains(&"*".to_owned()))
        {
            let tools = tools(role, &Registry::builtin(None), 1);
            assert!(tools.get(crate::tools::TASK).is_none());
            for name in &role.tools {
                let tool = tools.get(name).unwrap();
                let args = sample(&tool.spec().parameters);
                if let Ok(check) = project.check(tool.as_ref(), &args) {
                    for ask in check.asks {
                        assert!(
                            !permissions::changes_things(&ask.permission),
                            "{name}: {ask:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_users_file_overrides_the_built_in_of_its_id_and_may_add_one() {
        let (_dir, paths) = paths();
        let dir = paths.roles_dir();
        io::fs::make_dir(&dir).unwrap();
        let write = |id: &str, text: &str| {
            io::fs::write_atomic(
                &dir.join(format!("{id}.toml")),
                text.as_bytes(),
                io::fs::Access::Private,
            )
            .unwrap();
        };
        write(
            "reader",
            "id = \"reader\"\nsummary = \"Reads the CHANGELOG only.\"\ntools = [\"read\"]\nprompt = \"Be brief.\"\nmodel = \"fake-coder\"\n",
        );
        write(
            "tester",
            "id = \"tester\"\nsummary = \"Runs the tests.\"\ntools = [\"bash\"]\nprompt = \"Run them.\"\n",
        );
        let roles = load(&paths).unwrap();
        assert_eq!(roles.len(), builtin().len() + 1);
        let reader = find(&roles, "reader").unwrap();
        assert_eq!(reader.summary, "Reads the CHANGELOG only.");
        assert_eq!(reader.model.as_deref(), Some("fake-coder"));
        assert_eq!(roles.last().unwrap().id, "tester");
        // A file named other than its id is refused, so a role never overrides one by surprise.
        write(
            "wrong",
            "id = \"other\"\nsummary = \"x\"\ntools = [\"read\"]\nprompt = \"x\"\n",
        );
        let err = load(&paths).unwrap_err().to_string();
        assert!(err.contains("named for its id"), "{err}");
    }

    #[test]
    fn a_roles_binding_is_the_users_choice_then_its_file_then_the_session() {
        let (_dir, paths) = paths();
        let catalog = catalog();
        let (session, effort) = (catalog.get("session").unwrap(), Some("high"));
        let role = |model: Option<&str>, effort: Option<&str>| Role {
            id: "reader".into(),
            summary: String::new(),
            tools: vec!["read".into()],
            except: Vec::new(),
            prompt: String::new(),
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        };
        // Nothing chosen: the session's own.
        let (got, level) = role(None, None).binding(&catalog, &paths, (session, effort));
        assert_eq!(got.id, "session");
        assert_eq!(level.as_deref(), effort);
        // The file's pin.
        let (got, level) =
            role(Some("pinned"), Some("low")).binding(&catalog, &paths, (session, effort));
        assert_eq!(got.id, "pinned");
        assert_eq!(level.as_deref(), Some("low"));
        // The user's choice wins over it.
        prefs::remember_role(
            &paths,
            "reader",
            prefs::Choice {
                model: Some("chosen".into()),
                effort: None,
            },
        );
        let (got, level) =
            role(Some("pinned"), Some("low")).binding(&catalog, &paths, (session, effort));
        assert_eq!(got.id, "chosen");
        assert_eq!(level.as_deref(), Some("low"));
        // A pin crowbot no longer serves gives way to the session's model.
        prefs::remember_role(
            &paths,
            "reader",
            prefs::Choice {
                model: Some("gone-model".into()),
                effort: None,
            },
        );
        let (got, _) = role(Some("pinned"), None).binding(&catalog, &paths, (session, effort));
        assert_eq!(got.id, "session");
    }
}
