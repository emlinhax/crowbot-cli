//! The kinds of subagent the `task` tool starts: one TOML file per kind in data/agents/,
//! registered in `SOURCES`. Each says what it is for, which of the parent's tools it gets, and
//! what it adds to the parent's system prompt. Every kind runs under the parent's mode.
// CEILING: a kind cannot be stricter than the mode the user chose. Upgrade: a `mode = "<id>"` key
// giving its subagent a mode cell of its own.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::config::limits;
use crate::data;
use crate::tools::Registry;

const SOURCES: &[&str] = &[data::raw!("agents/explore.toml")];

static KINDS: LazyLock<Vec<Kind>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|src| data::parse(src, "agents/*.toml"))
        .collect()
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kind {
    pub id: String,
    pub summary: String,
    /// The parent's tools it gets, by name.
    tools: Vec<String>,
    pub prompt: String,
}

pub fn all() -> &'static [Kind] {
    &KINDS
}

pub fn get(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// The tools a subagent of `kind` gets from its parent's `registry`, `depth` runs below the
/// frontend's: never `task` at the depth limit, so it is never offered a call it could not make.
pub fn tools(kind: &Kind, registry: &Registry, depth: usize) -> Registry {
    let deepest = depth >= limits::get().subagent.max_depth.value;
    let names: Vec<&str> = kind
        .tools
        .iter()
        .map(String::as_str)
        .filter(|name| !(deepest && *name == crate::tools::TASK))
        .collect();
    registry.subset(&names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::catalog;
    use crate::tools::testing::{Project, sample};

    #[test]
    fn kinds_have_unique_ids_and_name_real_tools() {
        let registry = Registry::builtin(None);
        let mut ids: Vec<&str> = all().iter().map(|k| k.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all().len());
        for kind in all() {
            for tool in &kind.tools {
                assert!(registry.get(tool).is_some(), "{}: no tool {tool}", kind.id);
            }
        }
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
