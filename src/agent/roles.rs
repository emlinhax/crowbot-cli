//! The roles the `task` tool can start. Three are built in — one TOML file each in data/agents/,
//! registered in `SOURCES` — and the user adds or overrides one by dropping a file of the same
//! shape in ~/.crowbot/agents/, named for its `id`. Each says what it is for, which of the
//! parent's tools it gets, what it adds to the system prompt, and optionally the model and effort
//! it runs on instead of the session's. Every role runs under the parent's mode.
// CEILING: a role cannot be stricter than the mode the user chose. Upgrade: a `mode = "<id>"` key
// giving its subagent a mode cell of its own.

use std::path::PathBuf;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

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

/// The comment every role file `/models` writes opens with.
static HEADER: LazyLock<Header> = data::load!("agents.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    text: String,
    fallback: Fallback,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fallback {
    id: String,
    summary: String,
}

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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub id: String,
    pub summary: String,
    /// The parent's tools it gets, by name; `*` is all of them.
    pub tools: Vec<String>,
    /// Tools a `*` leaves out.
    #[serde(default)]
    pub except: Vec<String>,
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

/// Every role this machine has: the built-in ones the user did not delete, then each file in
/// ~/.crowbot/agents/, which overrides the built-in of its id and may add one of its own.
pub fn load(paths: &Paths) -> anyhow::Result<Vec<Role>> {
    let deleted = prefs::load(paths).hidden;
    let mut roles: Vec<Role> = builtin()
        .iter()
        .filter(|role| !deleted.contains(&role.id))
        .cloned()
        .collect();
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

/// The line the model is told of one role: what it is for, and the model it runs on where that is
/// not the session's, since a cheap reader against a strong worker is a cost decision too.
pub fn line(role: &Role, paths: &Paths, model: &str, effort: Option<&str>) -> String {
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
}

/// The role a dispatch falls back to when the user deleted every one of them: the session's own
/// model, effort and tools, under the name data/agents.toml gives it, so `task` still works for a
/// machine that wants no roles of its own.
pub fn fallback() -> Role {
    let mut role: Role = data::parse(data::raw!("agents/_template.toml"), "agents/_template.toml");
    role.id = HEADER.fallback.id.clone();
    role.summary = HEADER.fallback.summary.clone();
    role
}

/// The roles a dispatch may name: the machine's own, or the one fallback role when it has none.
pub fn dispatchable(paths: &Paths) -> Vec<Role> {
    let roles = load(paths).unwrap_or_default();
    if roles.is_empty() {
        vec![fallback()]
    } else {
        roles
    }
}

/// The system prompt's section naming every role, each as `line` words it.
pub fn section(paths: &Paths, model: &str, effort: Option<&str>) -> Option<String> {
    // With no role of its own the machine still dispatches, so the fallback is named too.
    let roles = dispatchable(paths);
    let lines: Vec<String> = roles
        .iter()
        .map(|role| line(role, paths, model, effort))
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

/// Whether `id` can name a role: it becomes a file name, and the model is told it.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A role about to be added: the built-in template's prompt and tools, with an empty summary for
/// the user to write.
pub fn new_role(id: &str, everything: bool) -> Role {
    let mut role: Role = data::parse(data::raw!("agents/_template.toml"), "agents/_template.toml");
    role.id = id.to_owned();
    role.summary = String::new();
    if !everything {
        role.tools = vec![
            "read".into(),
            "grep".into(),
            "glob".into(),
            "webfetch".into(),
        ];
        role.except = Vec::new();
    }
    role
}

/// Writes `role`'s file into the user's folder: what `/models` changes, and what a new role is.
/// The file is the source, so a built-in role edited here keeps the edit across sessions.
pub fn save(paths: &Paths, role: &Role) -> anyhow::Result<PathBuf> {
    if !valid_id(&role.id) {
        anyhow::bail!("a role's name is letters, digits, `-` and `_`");
    }
    let dir = paths.roles_dir();
    io::fs::make_dir(&dir)?;
    let file = dir.join(format!("{}.toml", role.id));
    let text = format!("{}{}", HEADER.text, file_text(role));
    io::fs::write_atomic(&file, text.as_bytes(), io::fs::Access::Private)?;
    Ok(file)
}

/// Deletes a role: a user's file goes, and a built-in one is remembered as deleted, since its
/// file ships with the build.
pub fn remove(paths: &Paths, role: &Role) -> anyhow::Result<()> {
    let file = paths.roles_dir().join(format!("{}.toml", role.id));
    if io::fs::exists(&file) {
        io::fs::remove(&file)?;
    }
    if builtin().iter().any(|built| built.id == role.id) {
        prefs::hide_role(paths, &role.id);
    }
    Ok(())
}

/// The role's file as text: the keys the loader reads, its prompt in a literal block so it stays
/// readable by hand.
fn file_text(role: &Role) -> String {
    let quoted = |text: &str| toml::Value::String(text.to_owned()).to_string();
    let list = |items: &[String]| {
        items
            .iter()
            .map(|item| quoted(item))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = format!(
        "id = {}\nsummary = {}\ntools = [{}]\n",
        quoted(&role.id),
        quoted(&role.summary),
        list(&role.tools)
    );
    if !role.except.is_empty() {
        out.push_str(&format!("except = [{}]\n", list(&role.except)));
    }
    let prompt = role.prompt.trim();
    let fence = "'''";
    if prompt.contains(fence) {
        // A literal block cannot hold its own delimiter; an escaped string can hold anything.
        out.push_str(&format!("prompt = {}\n", quoted(prompt)));
    } else {
        // The fence closes on the prompt's last line, as the built-in files do, so the value has
        // no newline the prompt did not have.
        out.push_str(&format!("prompt = {fence}\n{prompt}{fence}\n"));
    }
    if let Some(model) = &role.model {
        out.push_str(&format!("model = {}\n", quoted(model)));
    }
    if let Some(level) = &role.effort {
        out.push_str(&format!("effort = {}\n", quoted(level)));
    }
    out
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
    fn a_machine_with_no_roles_dispatches_one_on_the_sessions_own_settings() {
        let (_dir, paths) = paths();
        assert_eq!(load(&paths).unwrap().len(), builtin().len());
        for role in builtin() {
            prefs::hide_role(&paths, &role.id);
        }
        assert!(load(&paths).unwrap().is_empty());
        let roles = dispatchable(&paths);
        assert_eq!(roles.len(), 1);
        let only = &roles[0];
        // It is the fallback role, and it pins nothing: the session's model and effort apply.
        assert_eq!(only.id, HEADER.fallback.id);
        assert!(only.model.is_none() && only.effort.is_none());
        assert_eq!(only.tools, vec!["*".to_owned()]);
        // The prompt still names it, so the agent knows what to call.
        let section = section(&paths, "m", None).unwrap();
        assert!(
            section.contains(&format!("`{}`", HEADER.fallback.id)),
            "{section}"
        );
        // Adding a role of the machine's own puts the fallback away.
        let mut role = new_role("tester", true);
        role.summary = "Tests.".into();
        save(&paths, &role).unwrap();
        assert_eq!(dispatchable(&paths).len(), 1);
        assert_eq!(dispatchable(&paths)[0].id, "tester");
    }

    #[test]
    fn a_saved_role_is_the_file_the_loader_reads_and_stays_readable() {
        let (_dir, paths) = paths();
        let mut role = new_role("tester", true);
        role.summary = "Runs the tests.".into();
        role.prompt = "Run them.\nThen say what failed.".into();
        let file = save(&paths, &role).unwrap();
        let text = io::fs::read_string(&file).unwrap().unwrap();
        // The header says where the keys are explained, and the prompt keeps its lines.
        assert!(text.starts_with("# "), "{text}");
        assert!(
            text.contains("prompt = '''\nRun them.\nThen say what failed.'''"),
            "{text}"
        );
        let loaded = load(&paths).unwrap();
        let back = find(&loaded, "tester").unwrap();
        assert_eq!(back.summary, "Runs the tests.");
        assert_eq!(back.prompt, "Run them.\nThen say what failed.");
        assert_eq!(back.tools, vec!["*".to_owned()]);
        // A prompt holding the literal block's own delimiter is written as a quoted string.
        let mut odd = role.clone();
        odd.prompt = "It's ''' here".into();
        save(&paths, &odd).unwrap();
        let back = load(&paths).unwrap();
        assert_eq!(find(&back, "tester").unwrap().prompt, "It's ''' here");
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
