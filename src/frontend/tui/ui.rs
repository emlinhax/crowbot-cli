//! Typed access to data/ui.toml.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::text::units::Plural;

static UI: LazyLock<Ui> = data::load!("ui.toml");

pub fn get() -> &'static Ui {
    &UI
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    pub spinner: Vec<String>,
    pub prompt: String,
    pub user_mark: String,
    pub reply_mark: String,
    pub placeholder: String,
    pub tips: Vec<String>,
    pub marks: Marks,
    pub footer: Footer,
    pub welcome: WelcomeText,
    pub text: Text,
    pub outcome: OutcomeText,
    pub card: CardText,
    pub palette: PaletteText,
    pub thinking: ThinkingText,
    pub status: StatusText,
    pub models: ModelsText,
    pub resume: ResumeText,
    pub undo: UndoText,
    pub task: TaskText,
    pub login: LoginText,
    pub replace: ReplaceText,
    pub signup: SignupText,
    pub account: AccountText,
    pub forums: ForumsText,
}

/// Marks drawn across the session; each is one cell wide.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marks {
    pub done: String,
    pub selected: String,
    pub field: String,
    pub answered: String,
    pub ticked: String,
    pub unticked: String,
    pub queued: String,
    pub mask: String,
    pub rule: String,
    pub side: String,
    pub top_left: String,
    pub top_right: String,
    pub bottom_left: String,
    pub bottom_right: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WelcomeText {
    pub version: String,
    pub separator: String,
    pub modes: String,
}

/// How a run that ended early is told.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeText {
    /// `{stopped}` is the shared turn-limit line (data/frontend.toml).
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
    pub copy_code: String,
    pub waiting: String,
    pub checking: String,
    pub failed: String,
    pub hint: String,
}

/// The question before a new key replaces the stored one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceText {
    pub title: String,
    pub account: String,
    pub device: String,
    pub unreadable: String,
    pub replace: String,
    pub keep: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignupText {
    pub title: String,
    pub number: String,
    pub done: String,
    pub closed: String,
    pub failed: String,
    pub retry: String,
    pub close: String,
    pub hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountText {
    pub title: String,
    pub loading: String,
    pub revoking: String,
    pub keys: String,
    pub close: String,
    pub back: String,
    pub key: String,
    pub current: String,
    pub current_kept: String,
    pub confirm: String,
    pub device_warning: String,
    pub account_warning: String,
    pub revoke: String,
    pub keep: String,
    pub revoked: String,
    pub failed: String,
    pub hint: String,
    pub usage: String,
    pub usage_model: String,
    pub requests: Plural,
    pub saving: String,
    pub fallback: String,
    pub fallback_set: String,
    pub on: String,
    pub off: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForumsText {
    pub title: String,
    pub add: String,
    pub hint: String,
    pub menu_hint: String,
    pub log_in: String,
    pub log_in_again: String,
    pub log_out: String,
    pub remove: String,
    pub add_title: String,
    pub add_note: String,
    pub search: String,
    pub found_title: String,
    pub found_hint: String,
    pub no_match: String,
    pub login_title: String,
    pub login_note: String,
    pub username: String,
    pub password: String,
    pub input_hint: String,
    pub searching: String,
    pub adding: String,
    pub logging_in: String,
    pub saving: String,
    pub cancel_hint: String,
    pub failed: String,
}

/// How a `task` card shows its subagent at work.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskText {
    pub step: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoText {
    pub done: String,
    pub restored: Plural,
    pub not_restored: String,
    pub failed: String,
    pub kept: String,
    pub off: String,
    pub nothing: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeText {
    pub title: String,
    pub hint: String,
    pub none: String,
    pub saved: String,
}

/// `/models`: one tab per agent, each with its model, effort and what it is for.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsText {
    pub title: String,
    pub main: String,
    pub new: String,
    pub tab_separator: String,
    pub model: String,
    pub effort: String,
    pub summary: String,
    pub summary_hint: String,
    pub prompt: String,
    pub prompt_hint: String,
    pub more_lines: String,
    pub tools: String,
    pub delete: String,
    pub name: String,
    pub create: String,
    pub follow_model: String,
    pub follow_effort: String,
    pub follow_hint: String,
    pub effort_default: String,
    pub session_model: String,
    pub role_model: String,
    pub role_effort: String,
    pub role_session: String,
    pub switched: String,
    pub effort_set: String,
    pub tools_all: String,
    pub tools_all_but: String,
    pub name_hint: String,
    pub create_hint: String,
    pub edit_summary: String,
    pub edit_prompt: String,
    pub edit_name: String,
    pub field_prompt: String,
    pub delete_hint: String,
    pub delete_title: String,
    pub delete_built_in: String,
    pub delete_file: String,
    pub hint: String,
    pub targets_hint: String,
    pub input_hint: String,
    pub confirm_hint: String,
    pub title_model: String,
    pub told: String,
    pub added: String,
    pub system_prompt: String,
    pub system_prompt_note: String,
    pub saved: String,
    pub created: String,
    pub deleted: String,
    pub name_taken: String,
    pub name_bad: String,
    pub summary_bad: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusText {
    pub frames: Vec<String>,
    pub plain: Vec<String>,
    pub verbs: Vec<String>,
    pub tokens: String,
    pub working: String,
    pub retry: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingText {
    pub closed: String,
    pub open: String,
    pub done: String,
    pub untimed: String,
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
    pub questions_title: String,
    pub question_tab: String,
    pub review_tab: String,
    pub review_title: String,
    pub own_words: String,
    pub unanswered: String,
    pub question_hint: String,
    pub multiple_hint: String,
    pub review_hint: String,
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

/// What the footer can show, in data/ui.toml's order.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FooterItem {
    Mode,
    /// The model, with the effort it is sent.
    Model,
    Context,
    Cost,
    /// What is left of the plan's allowance, when the account holds one.
    Allowance,
    Balance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footer {
    pub items: Vec<FooterItem>,
    pub separator: String,
    pub context: String,
    pub cost: String,
    pub allowance: String,
    pub balance: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub queued: String,
    pub steering: String,
    pub request: String,
    pub not_logged_in: String,
    pub key_rejected: String,
    pub resumed: String,
    pub declined: String,
    pub ctrl_c_again: String,
    pub unknown_command: String,
    /// Under a card cut to fit: how many lines it left out.
    pub more_lines: Plural,
    pub more_below: String,
    pub saved: String,
    pub copied: String,
    pub nothing_to_copy: String,
    pub busy: String,
    pub effort_default: String,
    pub effort_unsent: String,
    pub new_conversation: String,
    pub new_saved: String,
    pub spent: String,
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
