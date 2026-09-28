use serde::Serialize;

use crate::mode::Mode;
use crate::permission::rule::{self, Action, Rule};
use crate::text::template;

/// One thing a tool call needs cleared: a permission and the targets it applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ask {
    pub permission: String,
    pub patterns: Vec<String>,
    /// What "always allow" would add as rules; the patterns themselves when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub always: Vec<String>,
}

impl Ask {
    pub fn new(permission: &str, pattern: impl Into<String>) -> Self {
        Self {
            permission: permission.to_owned(),
            patterns: vec![pattern.into()],
            always: Vec::new(),
        }
    }

    /// Rules that make the same ask pass without a prompt next time.
    pub fn always_rules(&self) -> Vec<Rule> {
        let patterns = if self.always.is_empty() {
            &self.patterns
        } else {
            &self.always
        };
        patterns
            .iter()
            .map(|pattern| Rule {
                permission: self.permission.clone(),
                pattern: pattern.clone(),
                action: Action::Allow,
            })
            .collect()
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
    /// Defaults, then user config, then project config.
    pub rules: &'a [Rule],
    /// "Always" answers given during this session.
    pub approved: &'a [Rule],
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
                let layers = self.rules.iter().chain(self.approved).chain(&locks);
                match rule::evaluate(layers, &ask.permission, pattern) {
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

    fn policy<'a>(mode: &'a str, rules: &'a [Rule], approved: &'a [Rule]) -> Policy<'a> {
        Policy {
            rules,
            approved,
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
            (
                "plan",
                Ask::new("bash", "cargo build"),
                Decision::Deny("bash cargo build".into()),
            ),
        ];
        for (mode, ask, expected) in cases {
            let got = policy(mode, &rules, &[]).decide(std::slice::from_ref(&ask));
            assert_eq!(got, expected, "{mode} {ask:?}");
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
            policy("manual", &rules, &[]).decide(&ask),
            Decision::Deny(_)
        ));
        assert_eq!(policy("auto", &rules, &[]).decide(&ask), Decision::Allow);
    }

    #[test]
    fn approvals_pass_in_manual_but_cannot_lift_plan_locks() {
        let rules = defaults();
        let ask = Ask::new("bash", "cargo build");
        let approved = ask.always_rules();
        assert_eq!(
            policy("manual", &rules, &approved).decide(std::slice::from_ref(&ask)),
            Decision::Allow
        );
        assert!(matches!(
            policy("plan", &rules, &approved).decide(&[ask]),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn one_denied_target_denies_the_call() {
        let rules = defaults();
        let asks = [
            Ask::new("bash", "git status"),
            Ask::new("bash_write", "echo x > f"),
        ];
        assert!(matches!(
            policy("plan", &rules, &[]).decide(&asks),
            Decision::Deny(_)
        ));
    }
}
