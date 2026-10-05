use serde::Serialize;

use crate::mode::Mode;
use crate::permission::rule::{self, Action, Rule};
use crate::text::template;

/// One thing a tool call needs cleared: a permission and the targets it applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ask {
    pub permission: String,
    pub patterns: Vec<String>,
}

impl Ask {
    pub fn new(permission: &str, pattern: impl Into<String>) -> Self {
        Self {
            permission: permission.to_owned(),
            patterns: vec![pattern.into()],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refused outright; names what was refused.
    Deny(String),
    Ask,
}

/// Everything a decision depends on, in the order rules are layered.
pub struct Policy<'a> {
    /// Defaults, then user config, then project config. Permanent allowances live here.
    pub rules: &'a [Rule],
    pub mode: &'a Mode,
    pub plan_file: &'a str,
}

impl Policy<'_> {
    pub fn decide(&self, asks: &[Ask]) -> Decision {
        let locks: Vec<Rule> = self
            .mode
            .lock
            .iter()
            .map(|r| Rule {
                pattern: template::fill(&r.pattern, &[("plan_file", self.plan_file)]),
                ..r.clone()
            })
            .collect();
        let mut denied = None;
        let mut asking = false;
        for ask in asks {
            for pattern in &ask.patterns {
                // A lock can tighten the rules but never lift one of their denies.
                let action = match (
                    rule::evaluate(self.rules, &ask.permission, pattern),
                    rule::last_match(&locks, &ask.permission, pattern),
                ) {
                    (Action::Deny, _) => Action::Deny,
                    (_, Some(lock)) => lock,
                    (from_rules, None) => from_rules,
                };
                match action {
                    Action::Deny => {
                        denied.get_or_insert_with(|| format!("{} {pattern}", ask.permission));
                    }
                    Action::Ask => asking = true,
                    Action::Allow => {}
                }
            }
        }
        let verdict = match (&denied, asking) {
            (Some(_), _) => self.mode.verdicts.deny,
            (None, true) => self.mode.verdicts.ask,
            (None, false) => Action::Allow,
        };
        match verdict {
            Action::Allow => Decision::Allow,
            Action::Ask => Decision::Ask,
            Action::Deny => Decision::Deny(denied.unwrap_or_else(|| describe(asks))),
        }
    }
}

pub fn describe(asks: &[Ask]) -> String {
    asks.iter()
        .map(|a| format!("{} {}", a.permission, a.patterns.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode;

    fn policy<'a>(mode: &'a str, rules: &'a [Rule]) -> Policy<'a> {
        Policy {
            rules,
            mode: mode::get(mode).unwrap(),
            plan_file: "/home/u/.crowbot/plans/s1.md",
        }
    }

    fn defaults() -> Vec<Rule> {
        crate::settings::default_rules()
    }

    #[test]
    fn mode_by_tool_matrix() {
        let rules = defaults();
        let cases = [
            // (mode, ask, expected)
            ("manual", Ask::new("read", "src/a.rs"), Decision::Allow),
            ("manual", Ask::new("read", ".env"), Decision::Ask),
            ("manual", Ask::new("search", ".env"), Decision::Ask),
            (
                "manual",
                Ask::new("search", "config/.env.local"),
                Decision::Ask,
            ),
            ("manual", Ask::new("search", "src"), Decision::Allow),
            ("manual", Ask::new("edit", "src/a.rs"), Decision::Ask),
            ("manual", Ask::new("bash", "cargo test"), Decision::Ask),
            ("auto", Ask::new("edit", "src/a.rs"), Decision::Allow),
            ("auto", Ask::new("bash", "rm -rf target"), Decision::Allow),
            ("plan", Ask::new("read", "src/a.rs"), Decision::Allow),
            ("plan", Ask::new("bash", "git status"), Decision::Allow),
            (
                "plan",
                Ask::new("edit", "src/a.rs"),
                Decision::Deny("edit src/a.rs".into()),
            ),
            (
                "plan",
                Ask::new("edit", "/home/u/.crowbot/plans/s1.md"),
                Decision::Allow,
            ),
            ("plan", Ask::new("bash", "cargo build"), Decision::Allow),
            (
                "plan",
                Ask::new("bash", "cargo run"),
                Decision::Deny("bash cargo run".into()),
            ),
            ("manual", Ask::new("webfetch", "docs.rs"), Decision::Ask),
            ("plan", Ask::new("webfetch", "docs.rs"), Decision::Allow),
        ];
        for (mode, ask, expected) in cases {
            let got = policy(mode, &rules).decide(std::slice::from_ref(&ask));
            assert_eq!(got, expected, "{mode} {ask:?}");
        }
    }

    #[test]
    fn plan_runs_inspection_and_checks_but_nothing_that_edits() {
        let rules = defaults();
        let cases = [
            ("git status", true),
            ("git diff", true),
            ("git diff HEAD~1 -- src", true),
            ("git log --oneline -5", true),
            ("git blame src/a.rs", true),
            ("git branch", true),
            ("git branch --contains HEAD", true),
            ("ls -la src", true),
            ("pwd", true),
            ("cd src", true),
            ("tree -L 2", true),
            ("find . -name '*.rs'", true),
            ("wc -l src/a.rs", true),
            ("cargo test --locked", true),
            ("cargo test 2>&1", true),
            ("cargo clippy --all-targets", true),
            ("make lint", true),
            ("npm test", true),
            ("rustc --version", true),
            ("git branch -D main", false),
            ("git branch feature", false),
            ("git tag v1", false),
            ("git blame --contents .env src/a.rs", false),
            ("git grep -O sh x", false),
            ("find . -delete", false),
            ("find . -exec rm {} ;", false),
            ("tree -o out.txt", false),
            ("cargo clippy --fix --allow-dirty", false),
            ("cargo run", false),
            ("cargo fmt", false),
            ("npm test -- -u", false),
            ("make install", false),
            ("env", false),
            ("git difftool --extcmd='rm -rf ~' -y", false),
            ("git diff --output=/home/u/.bashrc HEAD~1", false),
            ("git log -p --ext-diff", false),
            ("git diff --no-index /etc/passwd x", false),
            ("rg --pre=/tmp/evil --pre-glob '*' x .", false),
            ("cat .env", false),
            ("lsof -i", false),
        ];
        for (command, allowed) in cases {
            let got = policy("plan", &rules).decide(&[Ask::new("bash", command)]);
            assert_eq!(got == Decision::Allow, allowed, "{command}: {got:?}");
        }
    }

    #[test]
    fn user_denies_hold_in_manual_but_not_in_auto() {
        let mut rules = defaults();
        rules.push(Rule {
            permission: "bash".into(),
            pattern: "rm *".into(),
            action: Action::Deny,
        });
        let ask = [Ask::new("bash", "rm -rf x")];
        assert!(matches!(
            policy("manual", &rules).decide(&ask),
            Decision::Deny(_)
        ));
        assert_eq!(policy("auto", &rules).decide(&ask), Decision::Allow);
    }

    #[test]
    fn config_allowances_pass_in_manual_but_cannot_lift_plan_locks() {
        let mut rules = defaults();
        rules.push(Rule {
            permission: "bash".into(),
            pattern: "cargo *".into(),
            action: Action::Allow,
        });
        let ask = Ask::new("bash", "cargo run");
        assert_eq!(
            policy("manual", &rules).decide(std::slice::from_ref(&ask)),
            Decision::Allow
        );
        assert!(matches!(
            policy("plan", &rules).decide(&[ask]),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn a_plan_lock_that_allows_cannot_lift_a_user_deny() {
        let mut rules = defaults();
        rules.push(Rule {
            permission: "bash".into(),
            pattern: "git log *".into(),
            action: Action::Deny,
        });
        let ask = [Ask::new("bash", "git log -p")];
        assert!(matches!(
            policy("plan", &rules).decide(&ask),
            Decision::Deny(_)
        ));
        assert_eq!(
            policy("plan", &rules).decide(&[Ask::new("bash", "git status")]),
            Decision::Allow
        );
    }

    #[test]
    fn one_denied_target_denies_the_call() {
        let rules = defaults();
        let asks = [
            Ask::new("bash", "git status"),
            Ask::new("bash_write", "echo x > f"),
        ];
        assert!(matches!(
            policy("plan", &rules).decide(&asks),
            Decision::Deny(_)
        ));
    }
}
