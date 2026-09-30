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
    pub outcome: OutcomeText,
    pub card: CardText,
    pub palette: PaletteText,
    pub thinking: ThinkingText,
    pub status: StatusText,
    pub picker: PickerText,
    pub login: LoginText,
}

/// How a run that ended early is told.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeText {
    pub turn_limit: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginText {
    pub title: String,
    pub pair: String,
    pub number: String,
    pub starting: String,
    pub open: String,
    pub code: String,
    pub waiting: String,
    pub checking: String,
    pub failed: String,
    pub hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickerText {
    pub title: String,
    pub hint: String,
    pub switched: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusText {
    pub wings: Vec<String>,
    pub plain: Vec<String>,
    pub verbs: Vec<String>,
    pub tokens: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingText {
    pub closed: String,
    pub open: String,
    pub done: String,
    pub live: String,
    pub gutter: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteText {
    pub title: String,
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
    pub separator: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub interrupt: String,
    pub retrying: String,
    pub queued: String,
    pub steering: String,
    pub cost_estimate: String,
    pub not_logged_in: String,
    pub interrupted: String,
    pub declined: String,
    pub cut_off: String,
    pub ctrl_c_again: String,
    pub unknown_command: String,
    pub elsewhere: String,
    pub more_below: String,
    pub saved: String,
    pub unsaved: String,
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
    fn parses() {
        let ui = super::get();
        assert!(!ui.spinner.is_empty());
        assert!(!ui.footer.items.is_empty());
    }
}
