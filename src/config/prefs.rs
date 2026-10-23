//! The model and effort the user last chose in a session, remembered across sessions in
//! ~/.crowbot/prefs.json, and the same choice for each role the `task` tool can start. A fresh
//! install has none and falls back to the configured default; a remembered choice overrides the
//! config files but not a one-off `--model`/`--effort` flag (settings::load layers it just below
//! the flags). A file that does not load, or an effort no longer known, is treated as none, so a
//! stale or hand-broken file only loses the memory.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::paths::Paths;
use crate::{effort, io};

#[derive(Default, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Each role's own model and effort, by role id; absent means the role's file, else the
    /// session's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, Choice>,
    /// Built-in roles the user deleted: their files ship with the build, so the deletion is a
    /// choice kept here. A role added again with the same id leaves this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

/// What one role runs on, either part optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// The remembered choices, or none when there is no file, it will not load, or its effort is one
/// no model offers any more.
pub fn load(paths: &Paths) -> Prefs {
    let mut prefs: Prefs = io::fs::read_json(&paths.prefs())
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Some(level) = &prefs.effort
        && effort::get(level).is_none()
    {
        prefs.effort = None;
    }
    prefs.roles.retain(|_, choice| {
        if let Some(level) = &choice.effort
            && effort::get(level).is_none()
        {
            choice.effort = None;
        }
        choice.model.is_some() || choice.effort.is_some()
    });
    prefs
}

/// The model and effort alone, as a settings layer: the roles' choices and deletions are not
/// settings.
pub fn settings(paths: &Paths) -> Prefs {
    Prefs {
        roles: BTreeMap::new(),
        hidden: Vec::new(),
        ..load(paths)
    }
}

/// Whether the user deleted `role` from the list.
pub fn hidden(paths: &Paths, role: &str) -> bool {
    load(paths).hidden.iter().any(|id| id == role)
}

/// Remembers that `role` is deleted, keeping every other role's state.
pub fn hide_role(paths: &Paths, role: &str) {
    let mut prefs = load(paths);
    if !prefs.hidden.iter().any(|id| id == role) {
        prefs.hidden.push(role.to_owned());
    }
    prefs.roles.remove(role);
    save(paths, &prefs);
}

/// Forgets that `role` was deleted, so a built-in of that id shows again.
pub fn unhide_role(paths: &Paths, role: &str) {
    let mut prefs = load(paths);
    prefs.hidden.retain(|id| id != role);
    save(paths, &prefs);
}

/// Remembers `model`, keeping the effort already remembered.
pub fn remember_model(paths: &Paths, model: &str) {
    let mut prefs = load(paths);
    prefs.model = Some(model.to_owned());
    save(paths, &prefs);
}

/// Forgets the effort, so the model's own default applies again.
pub fn forget_effort(paths: &Paths) {
    let mut prefs = load(paths);
    prefs.effort = None;
    save(paths, &prefs);
}

/// Remembers `effort`, keeping the model already remembered.
pub fn remember_effort(paths: &Paths, effort: &str) {
    let mut prefs = load(paths);
    prefs.effort = Some(effort.to_owned());
    save(paths, &prefs);
}

/// Remembers what `role` runs on from the next dispatch on, keeping every other role's choice.
pub fn remember_role(paths: &Paths, role: &str, choice: Choice) {
    let mut prefs = load(paths);
    if choice.model.is_none() && choice.effort.is_none() {
        prefs.roles.remove(role);
    } else {
        prefs.roles.insert(role.to_owned(), choice);
    }
    save(paths, &prefs);
}

/// A failed write is not worth interrupting a session; the choice just is not remembered.
fn save(paths: &Paths, prefs: &Prefs) {
    let _ = io::fs::write_json(&paths.prefs(), prefs, io::fs::Access::Private);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        (dir, paths)
    }

    #[test]
    fn a_remembered_model_and_effort_come_back_and_each_keeps_the_other() {
        let (_dir, paths) = paths();
        assert!(load(&paths).model.is_none() && load(&paths).effort.is_none());
        remember_model(&paths, "glm-5.3-flash-crow");
        remember_effort(&paths, effort::ids()[0]);
        remember_model(&paths, "crow-2");
        let prefs = load(&paths);
        assert_eq!(prefs.model.as_deref(), Some("crow-2"));
        assert_eq!(prefs.effort.as_deref(), Some(effort::ids()[0]));
    }

    #[test]
    fn a_roles_choice_is_remembered_apart_from_the_sessions() {
        let (_dir, paths) = paths();
        remember_model(&paths, "crow-2");
        let low = effort::ids()[0];
        remember_role(
            &paths,
            "reader",
            Choice {
                model: Some("deepseek-v4.1-flash".into()),
                effort: Some(low.into()),
            },
        );
        remember_role(
            &paths,
            "worker",
            Choice {
                model: Some("glm-5.3-flash-crow".into()),
                effort: None,
            },
        );
        let prefs = load(&paths);
        assert_eq!(prefs.model.as_deref(), Some("crow-2"));
        assert_eq!(
            prefs.roles["reader"].model.as_deref(),
            Some("deepseek-v4.1-flash")
        );
        assert_eq!(prefs.roles["reader"].effort.as_deref(), Some(low));
        assert!(prefs.roles["worker"].effort.is_none());
        assert_eq!(
            prefs.roles["worker"].model.as_deref(),
            Some("glm-5.3-flash-crow")
        );
        // The settings layer carries the session's choice only.
        assert!(settings(&paths).roles.is_empty());
        assert_eq!(settings(&paths).model.as_deref(), Some("crow-2"));
        // Clearing both parts forgets the role.
        remember_role(&paths, "worker", Choice::default());
        assert!(!load(&paths).roles.contains_key("worker"));
    }

    #[test]
    fn a_deleted_role_is_remembered_and_can_come_back() {
        let (_dir, paths) = paths();
        remember_role(
            &paths,
            "reader",
            Choice {
                model: Some("crow-2".into()),
                effort: None,
            },
        );
        hide_role(&paths, "reader");
        assert!(hidden(&paths, "reader"));
        // Its own choice goes with it, and the setting layer carries neither.
        assert!(!load(&paths).roles.contains_key("reader"));
        assert!(settings(&paths).hidden.is_empty());
        hide_role(&paths, "reader");
        assert_eq!(load(&paths).hidden, vec!["reader".to_owned()]);
        unhide_role(&paths, "reader");
        assert!(!hidden(&paths, "reader"));
    }

    #[test]
    fn an_effort_no_model_offers_any_more_is_dropped_on_load() {
        let (_dir, paths) = paths();
        io::fs::write_json(
            &paths.prefs(),
            &serde_json::json!({
                "model": "crow-2",
                "effort": "bogus-level",
                "roles": {"reader": {"model": "crow-2", "effort": "bogus-level"}},
            }),
            io::fs::Access::Private,
        )
        .unwrap();
        let prefs = load(&paths);
        assert_eq!(prefs.model.as_deref(), Some("crow-2"));
        assert!(prefs.effort.is_none());
        let reader = prefs.roles.get("reader").unwrap();
        assert!(reader.effort.is_none());
        assert_eq!(reader.model.as_deref(), Some("crow-2"));
    }
}
