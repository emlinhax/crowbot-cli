//! The model and effort the user last chose in a session, remembered across sessions in
//! ~/.crowbot/prefs.json. A fresh install has none and falls back to the configured default; a
//! remembered choice overrides the config files but not a one-off `--model`/`--effort` flag
//! (settings::load layers it just below the flags). A file that does not load, or an effort no
//! longer known, is treated as none, so a stale or hand-broken file only loses the memory.

use serde::{Deserialize, Serialize};

use crate::config::paths::Paths;
use crate::config::subagents::{Role, Subagents};
use crate::{effort, io};

#[derive(Default, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Picked with `/models`; layered like `model`, a kind at a time.
    #[serde(default, skip_serializing_if = "Subagents::is_empty")]
    pub subagents: Subagents,
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
    prefs.subagents = prefs.subagents.cleaned();
    prefs
}

/// Remembers `model`, keeping the effort already remembered.
pub fn remember_model(paths: &Paths, model: &str) {
    let mut prefs = load(paths);
    prefs.model = Some(model.to_owned());
    save(paths, &prefs);
}

/// Remembers `effort`, keeping the model already remembered.
pub fn remember_effort(paths: &Paths, effort: &str) {
    let mut prefs = load(paths);
    prefs.effort = Some(effort.to_owned());
    save(paths, &prefs);
}

/// Remembers `role` for `kind` (every kind when `None`); an empty one forgets the entry, so the
/// config files' choice, or the inherited one, shows through again.
pub fn remember_role(paths: &Paths, kind: Option<&str>, role: Role) {
    let mut prefs = load(paths);
    prefs.subagents.set(kind, role);
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
    fn a_role_is_remembered_beside_the_chat_model_and_inherit_removes_its_key() {
        let (_dir, paths) = paths();
        remember_model(&paths, "crow-2");
        let explore = Role {
            model: Some("kimi-k3".into()),
            effort: Some(effort::ids()[0].into()),
        };
        remember_role(&paths, Some("explore"), explore.clone());
        let prefs = load(&paths);
        assert_eq!(prefs.model.as_deref(), Some("crow-2"));
        assert_eq!(prefs.subagents.role(Some("explore")), explore);
        let file: serde_json::Value = io::fs::read_json(&paths.prefs()).unwrap().unwrap();
        assert_eq!(file["subagents"]["kinds"]["explore"]["model"], "kimi-k3");

        let theirs = Role {
            model: Some("crow-2".into()),
            effort: None,
        };
        remember_role(&paths, Some("only-in-another-project"), theirs.clone());
        assert_eq!(
            load(&paths).subagents.role(Some("only-in-another-project")),
            theirs
        );
        remember_role(&paths, Some("only-in-another-project"), Role::default());
        remember_role(&paths, Some("explore"), Role::default());
        let file: serde_json::Value = io::fs::read_json(&paths.prefs()).unwrap().unwrap();
        assert!(file.get("subagents").is_none(), "{file}");
        assert_eq!(file["model"], "crow-2");
    }

    #[test]
    fn an_effort_no_model_offers_any_more_is_dropped_on_load() {
        let (_dir, paths) = paths();
        io::fs::write_json(
            &paths.prefs(),
            &serde_json::json!({"model": "crow-2", "effort": "bogus-level"}),
            io::fs::Access::Private,
        )
        .unwrap();
        let prefs = load(&paths);
        assert_eq!(prefs.model.as_deref(), Some("crow-2"));
        assert!(prefs.effort.is_none());
    }
}
