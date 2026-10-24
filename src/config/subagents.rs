//! Which model and effort subagents run on: one for every kind, one per kind (data/agents/), and
//! the models a `task` call may name. Read from the config files and prefs.json alike; every
//! field is optional, and an unset one inherits (agent/roles.rs resolves the chain).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::effort;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subagents {
    /// Every kind's model, unless its own entry names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The models a `task` call's `model` may name; unset, any model crowbot serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    /// By kind id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, Role>,
}

/// One role's own choices; `None` inherits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Subagents {
    /// The choices of `kind`, or of every kind when `None`.
    pub fn role(&self, kind: Option<&str>) -> Role {
        match kind {
            Some(id) => self.kinds.get(id).cloned().unwrap_or_default(),
            None => Role {
                model: self.model.clone(),
                effort: self.effort.clone(),
            },
        }
    }

    /// Replaces the choices of `kind` (every kind when `None`); an empty role removes the entry,
    /// so inheriting is the key's absence, never a stand-in value.
    pub fn set(&mut self, kind: Option<&str>, role: Role) {
        match kind {
            Some(id) if role == Role::default() => {
                self.kinds.remove(id);
            }
            Some(id) => {
                self.kinds.insert(id.to_owned(), role);
            }
            None => {
                self.model = role.model;
                self.effort = role.effort;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Every effort is a known level; `Err` names it. A kind no agent file here defines is kept
    /// and goes unused: a project's own kinds are named in the user's settings too.
    pub fn check(&self) -> anyhow::Result<()> {
        for effort in self.efforts() {
            effort::find(effort)?;
        }
        Ok(())
    }

    /// Drops what `check` would refuse, for a file crowbot wrote itself and a stale value in it
    /// should only lose that memory. Kinds stay, for the projects that define them.
    pub fn cleaned(mut self) -> Self {
        let known = |e: &Option<String>| e.as_deref().is_none_or(|e| effort::get(e).is_some());
        if !known(&self.effort) {
            self.effort = None;
        }
        for role in self.kinds.values_mut() {
            if !known(&role.effort) {
                role.effort = None;
            }
        }
        self.kinds.retain(|_, role| *role != Role::default());
        self
    }

    fn efforts(&self) -> impl Iterator<Item = &str> {
        self.effort
            .iter()
            .chain(self.kinds.values().filter_map(|r| r.effort.as_ref()))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(model: &str) -> Role {
        Role {
            model: Some(model.into()),
            effort: None,
        }
    }

    #[test]
    fn an_empty_role_removes_its_entry_rather_than_storing_a_stand_in() {
        let mut s = Subagents::default();
        s.set(Some("explore"), role("a"));
        assert_eq!(s.role(Some("explore")).model.as_deref(), Some("a"));
        s.set(Some("explore"), Role::default());
        assert!(s.kinds.is_empty() && s.is_empty());
        s.set(None, role("b"));
        assert_eq!(s.model.as_deref(), Some("b"));
        assert!(s.kinds.is_empty());
    }

    #[test]
    fn unknown_efforts_are_refused_and_dropped_but_another_projects_kinds_are_kept() {
        let mut s = Subagents::default();
        s.set(Some("only-in-another-project"), role("a"));
        assert!(s.check().is_ok());
        s.set(
            Some("explore"),
            Role {
                model: None,
                effort: Some("ludicrous".into()),
            },
        );
        s.effort = Some("ludicrous".into());
        assert!(format!("{}", s.check().unwrap_err()).contains("ludicrous"));
        let s = s.cleaned();
        assert!(
            s.effort.is_none() && !s.kinds.contains_key("explore"),
            "{s:?}"
        );
        assert_eq!(s.role(Some("only-in-another-project")), role("a"));
    }

    #[test]
    fn an_unset_role_writes_no_keys() {
        let text = toml::to_string(&Subagents::default()).unwrap();
        assert!(text.trim().is_empty(), "{text}");
    }
}
