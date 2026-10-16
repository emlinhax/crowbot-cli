use serde::{Deserialize, Serialize};

use crate::text::wildcard::wildcard;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

/// `pattern` matches what a tool asks to touch (a path, a command, a domain) with `*` and `?`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: Action,
}

/// The last matching rule wins, so later layers override earlier ones; no match means ask.
pub fn evaluate<'a>(
    rules: impl IntoIterator<Item = &'a Rule>,
    permission: &str,
    target: &str,
) -> Action {
    last_match(rules, permission, target).unwrap_or(Action::Ask)
}

pub fn last_match<'a>(
    rules: impl IntoIterator<Item = &'a Rule>,
    permission: &str,
    target: &str,
) -> Option<Action> {
    rules
        .into_iter()
        .filter(|r| wildcard(&r.permission, permission) && wildcard(&r.pattern, target))
        .last()
        .map(|r| r.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(permission: &str, pattern: &str, action: Action) -> Rule {
        Rule {
            permission: permission.into(),
            pattern: pattern.into(),
            action,
        }
    }

    #[test]
    fn last_match_wins_and_default_is_ask() {
        let rules = [
            rule("bash", "*", Action::Ask),
            rule("bash", "cargo *", Action::Allow),
            rule("bash", "cargo publish*", Action::Deny),
        ];
        assert_eq!(evaluate(&rules, "bash", "cargo test"), Action::Allow);
        assert_eq!(evaluate(&rules, "bash", "cargo publish"), Action::Deny);
        assert_eq!(evaluate(&rules, "bash", "rm x"), Action::Ask);
        assert_eq!(evaluate(&rules, "edit", "x"), Action::Ask);
    }
}
