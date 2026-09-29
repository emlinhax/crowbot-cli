//! Modes are data: each file in data/modes/ is one mode, and this list is the only place that
//! names them. Adding a mode is a TOML file plus one line below.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::permission::rule::{Action, Rule};

const SOURCES: &[&str] = &[
    include_str!("../data/modes/manual.toml"),
    include_str!("../data/modes/auto.toml"),
    include_str!("../data/modes/plan.toml"),
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
    #[allow(dead_code)] // read by the TUI (M3 step 3.5)
    pub color: String,
    /// Position in the Shift+Tab cycle.
    pub order: u32,
    pub summary: String,
    /// A prompt in data/prompts/ injected when the mode takes effect.
    #[serde(default)]
    pub reminder: Option<String>,
    pub verdicts: Verdicts,
    /// Rules applied after all others; `{plan_file}` is filled in per session.
    #[serde(default)]
    pub lock: Vec<Rule>,
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

#[cfg_attr(not(test), allow(dead_code))] // the Shift+Tab cycle (M3)
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
