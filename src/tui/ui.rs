//! Typed access to data/ui.toml.

use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/ui.toml");

static UI: LazyLock<Ui> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/ui.toml is checked by tests"));

pub fn get() -> &'static Ui {
    &UI
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    pub spinner: Vec<String>,
    pub prompt: String,
    pub placeholder: String,
    pub tips: Vec<String>,
    pub footer: Footer,
    pub text: Text,
    pub card: CardText,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardText {
    pub hint: String,
    pub input_hint: String,
    pub other: String,
    pub plan_title: String,
    pub plan_other: String,
    pub permission: Vec<PermissionChoice>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionChoice {
    pub label: String,
    pub reply: PermissionReply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionReply {
    Yes,
    No,
    /// No, and ask the user for the reason to pass on.
    NoWhy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footer {
    pub items: Vec<String>,
    pub drop: Vec<String>,
    pub separator: String,
    pub mode_hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub working: String,
    pub interrupt: String,
    pub retrying: String,
    pub queued: String,
    pub steering: String,
    pub thought: String,
    pub cost_estimate: String,
    pub not_logged_in: String,
    pub interrupted: String,
    pub declined: String,
    pub cut_off: String,
    pub ctrl_c_again: String,
    pub unknown_command: String,
    pub reasoning_shown: String,
    pub reasoning_hidden: String,
    pub saved: String,
    pub spent: String,
}

impl Ui {
    /// The spinner frame for `elapsed_ms` of work at `step_ms` per frame.
    pub fn spinner_frame(&self, elapsed_ms: u128, step_ms: u64) -> &str {
        let i = (elapsed_ms / u128::from(step_ms.max(1))) as usize % self.spinner.len().max(1);
        self.spinner.get(i).map_or("", String::as_str)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_and_drops_only_shown_items() {
        let ui = super::get();
        assert!(!ui.spinner.is_empty());
        for item in &ui.footer.drop {
            assert!(
                ui.footer.items.contains(item),
                "{item} dropped but never shown"
            );
        }
    }
}
