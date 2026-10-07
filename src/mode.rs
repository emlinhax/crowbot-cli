//! Modes are data: each file in data/modes/ is one mode, and this list is the only place that
//! names them. Adding a mode is a TOML file plus one line below.

use std::sync::LazyLock;

use serde::{Deserialize, Deserializer};

use crate::data;
use crate::permission::gate::{Policy, Verdicts};
use crate::permission::rule::{Action, Rule};
use crate::text::choices;

const SOURCES: &[&str] = &[
    data::raw!("modes/manual.toml"),
    data::raw!("modes/plan.toml"),
    data::raw!("modes/auto.toml"),
];

static MODES: LazyLock<Vec<Mode>> = LazyLock::new(|| {
    let mut modes: Vec<Mode> = SOURCES
        .iter()
        .map(|src| data::parse(src, "modes/*.toml"))
        .collect();
    modes.sort_by_key(|m| m.order);
    modes
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub id: String,
    pub label: String,
    /// A role in data/theme.toml for the mode's label and the editor border.
    pub color: String,
    /// Position in the Shift+Tab cycle.
    pub order: u32,
    pub summary: String,
    /// A prompt in data/prompts/ injected when the mode takes effect.
    #[serde(default)]
    pub reminder: Option<String>,
    pub verdicts: Verdicts,
    /// What the agent does when the model repeats the same call.
    pub doom_loop: DoomLoop,
    /// Rules applied after all others; `{plan_file}` is filled in per session.
    #[serde(default, deserialize_with = "locks")]
    pub lock: Vec<Rule>,
}

/// A lock as written: one pattern, a list of them, or commands that each match alone or followed
/// by arguments (`git log` and `git log -5`, never `git logx`). Expanded in file order, so the
/// last match still wins as it reads.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lock {
    permission: String,
    action: Action,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    commands: Vec<String>,
}

fn locks<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Rule>, D::Error> {
    let mut rules = Vec::new();
    for lock in Vec::<Lock>::deserialize(d)? {
        let commands = lock
            .commands
            .iter()
            .flat_map(|c| [c.clone(), format!("{c} *")]);
        let patterns: Vec<String> = lock
            .pattern
            .into_iter()
            .chain(lock.patterns)
            .chain(commands)
            .collect();
        if patterns.is_empty() {
            return Err(serde::de::Error::custom(format!(
                "a {} lock needs a pattern, patterns or commands",
                lock.permission
            )));
        }
        rules.extend(patterns.into_iter().map(|pattern| Rule {
            permission: lock.permission.clone(),
            pattern,
            action: lock.action,
        }));
    }
    Ok(rules)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoomLoop {
    Ask,
    /// Answer the repeated call with an error the model reads, and carry on.
    TellModel,
}

impl Mode {
    /// The gate's view of this mode over `rules`, for the session whose plan is `plan_file`.
    pub fn policy<'a>(&'a self, rules: &'a [Rule], plan_file: &'a str) -> Policy<'a> {
        Policy {
            rules,
            lock: &self.lock,
            verdicts: &self.verdicts,
            plan_file,
        }
    }
}

pub fn all() -> &'static [Mode] {
    &MODES
}

pub fn get(id: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.id == id)
}

/// `Err` lists the modes there are.
pub fn find(id: &str) -> anyhow::Result<&'static Mode> {
    get(id).ok_or_else(|| {
        let modes = MODES
            .iter()
            .map(|m| (m.id.as_str(), Some(format!("{}: {}", m.label, m.summary))));
        choices::unknown("mode", id, modes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::gate::{Ask, Decision};

    #[test]
    fn a_command_lock_matches_it_alone_or_with_arguments_in_file_order() {
        let mode: Mode = toml::from_str(
            r#"
            id = "t"
            label = "T"
            color = "muted"
            order = 9
            summary = "test"
            doom_loop = "ask"
            [verdicts]
            ask = "ask"
            deny = "deny"
            [[lock]]
            permission = "bash"
            pattern = "*"
            action = "deny"
            [[lock]]
            permission = "bash"
            action = "allow"
            commands = ["git log"]
            [[lock]]
            permission = "bash"
            action = "deny"
            patterns = ["git *--output*"]
            "#,
        )
        .unwrap();
        let patterns: Vec<&str> = mode.lock.iter().map(|r| r.pattern.as_str()).collect();
        assert_eq!(patterns, ["*", "git log", "git log *", "git *--output*"]);
    }

    #[test]
    fn modes_parse_with_unique_ids_and_known_reminders() {
        assert!(all().len() >= 3);
        for mode in all() {
            assert_eq!(all().iter().filter(|m| m.id == mode.id).count(), 1);
            if let Some(reminder) = &mode.reminder {
                assert!(
                    crate::agent::system_prompt::reminder(reminder).is_some(),
                    "{} names unknown reminder {reminder}",
                    mode.id
                );
            }
        }
    }

    fn policy<'a>(mode: &'a str, rules: &'a [Rule]) -> Policy<'a> {
        get(mode)
            .unwrap()
            .policy(rules, "/home/u/.crowbot/plans/s1.md")
    }

    fn defaults() -> Vec<Rule> {
        crate::config::settings::default_rules()
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
