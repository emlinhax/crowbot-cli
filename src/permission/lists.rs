//! Named shell command lists (data/command_lists.toml), written once for every rule and mode lock
//! that takes one by `list = "<name>"`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/command_lists.toml");

static LISTS: LazyLock<BTreeMap<String, List>> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/command_lists.toml is checked by tests"));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default)]
    commands: Vec<String>,
    #[serde(default)]
    patterns: Vec<String>,
}

/// The rule patterns list `name` stands for, or `None` when there is no such list.
pub fn patterns(name: &str) -> Option<Vec<String>> {
    let list = LISTS.get(name)?;
    Some(
        commands(&list.commands)
            .chain(list.patterns.iter().cloned())
            .collect(),
    )
}

/// Each command matches alone or followed by arguments: `git log` and `git log -5`, never
/// `git logx`.
pub fn commands(commands: &[String]) -> impl Iterator<Item = String> + '_ {
    commands.iter().flat_map(|c| [c.clone(), format!("{c} *")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_parse_and_commands_take_arguments() {
        let inspect = patterns("inspect").unwrap();
        assert!(inspect.iter().any(|p| p == "git log *"));
        assert!(inspect.iter().any(|p| p == "git branch -a"));
        assert!(!inspect.iter().any(|p| p == "git branch *"));
        assert!(patterns("checks").is_some() && patterns("unsafe_flags").is_some());
        assert!(patterns("nope").is_none());
    }
}
