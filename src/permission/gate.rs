use serde::{Deserialize, Serialize};

use crate::permission::rule::{self, Action, Rule};

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

/// What a mode makes of the rules' outcome.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdicts {
    /// What a rule's `ask` becomes.
    pub ask: Action,
    /// What a rule's `deny` becomes.
    pub deny: Action,
}

/// Everything a decision depends on, in the order rules are layered.
pub struct Policy<'a> {
    /// Defaults, then user config, then project config. Permanent allowances live here.
    pub rules: &'a [Rule],
    /// The mode's locks, applied after every rule.
    pub lock: &'a [Rule],
    pub verdicts: &'a Verdicts,
}

impl Policy<'_> {
    pub fn decide(&self, asks: &[Ask]) -> Decision {
        let mut denied = None;
        let mut asking = false;
        for ask in asks {
            for pattern in &ask.patterns {
                // A lock can tighten the rules but never lift one of their denies.
                let action = match (
                    rule::evaluate(self.rules, &ask.permission, pattern),
                    rule::last_match(self.lock, &ask.permission, pattern),
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
            (Some(_), _) => self.verdicts.deny,
            (None, true) => self.verdicts.ask,
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

    fn rule(permission: &str, pattern: &str, action: Action) -> Rule {
        Rule {
            permission: permission.into(),
            pattern: pattern.into(),
            action,
        }
    }

    const STRICT: Verdicts = Verdicts {
        ask: Action::Ask,
        deny: Action::Deny,
    };

    #[test]
    fn a_lock_tightens_the_rules_but_never_lifts_a_deny() {
        let rules = [
            rule("bash", "*", Action::Allow),
            rule("bash", "rm *", Action::Deny),
        ];
        let lock = [
            rule("bash", "*", Action::Deny),
            rule("bash", "rm *", Action::Allow),
        ];
        let policy = Policy {
            rules: &rules,
            lock: &lock,
            verdicts: &STRICT,
        };
        assert_eq!(
            policy.decide(&[Ask::new("bash", "ls")]),
            Decision::Deny("bash ls".into())
        );
        assert_eq!(
            policy.decide(&[Ask::new("bash", "rm x")]),
            Decision::Deny("bash rm x".into())
        );
    }

    #[test]
    fn verdicts_decide_what_an_ask_or_a_deny_becomes() {
        let rules = [
            rule("bash", "*", Action::Ask),
            rule("bash", "rm *", Action::Deny),
        ];
        let lenient = Verdicts {
            ask: Action::Allow,
            deny: Action::Allow,
        };
        for (verdicts, ls, rm) in [
            (&STRICT, Decision::Ask, Decision::Deny("bash rm x".into())),
            (&lenient, Decision::Allow, Decision::Allow),
        ] {
            let policy = Policy {
                rules: &rules,
                lock: &[],
                verdicts,
            };
            assert_eq!(policy.decide(&[Ask::new("bash", "ls")]), ls);
            assert_eq!(policy.decide(&[Ask::new("bash", "rm x")]), rm);
        }
    }

    #[test]
    fn one_denied_target_denies_the_call() {
        let rules = [rule("*", "*", Action::Allow)];
        let lock = [
            rule("edit", "*", Action::Deny),
            rule("edit", "/p/s1.md", Action::Allow),
        ];
        let policy = Policy {
            rules: &rules,
            lock: &lock,
            verdicts: &STRICT,
        };
        assert_eq!(
            policy.decide(&[Ask::new("edit", "/p/s1.md")]),
            Decision::Allow
        );
        let both = [Ask::new("edit", "/p/s1.md"), Ask::new("edit", "src/a.rs")];
        assert_eq!(policy.decide(&both), Decision::Deny("edit src/a.rs".into()));
    }
}
