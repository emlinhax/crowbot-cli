use serde::{Deserialize, Serialize};

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

/// `*` matches any run of characters (slashes included), `?` exactly one.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
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
    fn wildcards() {
        assert!(wildcard("*", ""));
        assert!(wildcard("git *", "git commit -m x"));
        assert!(!wildcard("git *", "gitk"));
        assert!(wildcard("src/*.rs", "src/a/b.rs"));
        assert!(wildcard("?.env", "a.env"));
        assert!(!wildcard("*.env", "a.env.example"));
        assert!(wildcard("a*b*c", "aXbYc"));
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
