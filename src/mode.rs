//! Modes are data: each file in data/modes/ is one mode, and this list is the only place that
//! names them. Adding a mode is a TOML file plus one line below.

use std::sync::LazyLock;

use serde::{Deserialize, Deserializer};

use crate::permission::rule::{Action, Rule};

const SOURCES: &[&str] = &[
    include_str!("../data/modes/manual.toml"),
    include_str!("../data/modes/plan.toml"),
    include_str!("../data/modes/auto.toml"),
];

static MODES: LazyLock<Vec<Mode>> = LazyLock::new(|| {
    let mut modes: Vec<Mode> = SOURCES
        .iter()
        .map(|src| toml::from_str(src).expect("data/modes/*.toml are checked by tests"))
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdicts {
    /// What a rule's `ask` becomes.
    pub ask: Action,
    /// What a rule's `deny` becomes.
    pub deny: Action,
    pub doom_loop: DoomLoop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoomLoop {
    Ask,
    /// Answer the repeated call with an error the model reads, and carry on.
    TellModel,
}

pub fn all() -> &'static [Mode] {
    &MODES
}

pub fn get(id: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.id == id)
}

/// `Err` lists the modes there are.
pub fn find(id: &str) -> Result<&'static Mode, String> {
    get(id).ok_or_else(|| {
        let choices: Vec<String> = MODES
            .iter()
            .map(|m| format!("{} ({}: {})", m.id, m.label, m.summary))
            .collect();
        format!("unknown mode `{id}`; choose one of: {}", choices.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_lock_matches_it_alone_or_with_arguments_in_file_order() {
        let mode: Mode = toml::from_str(
            r#"
            id = "t"
            label = "T"
            color = "muted"
            order = 9
            summary = "test"
            [verdicts]
            ask = "ask"
            deny = "deny"
            doom_loop = "ask"
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
}
