//! Typed access to data/model_text.toml, so a missing string fails the tests, not a session.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::text::template;

const SRC: &str = include_str!("../../data/model_text.toml");

static TEXT: LazyLock<ModelText> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/model_text.toml is checked by tests"));

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelText {
    pub no_result: String,
    pub cancelled: String,
    pub truncated_tool_call: String,
    pub unknown_tool: String,
    pub bad_json: String,
    pub invalid_arguments: String,
    pub denied: String,
    pub rejected: String,
    pub rejected_with_feedback: String,
    pub non_interactive: String,
    pub doom_loop: String,
    pub max_turns: String,
}

pub fn get() -> &'static ModelText {
    &TEXT
}

pub fn fill(text: &str, values: &[(&str, &str)]) -> String {
    template::fill(text, values)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        assert!(!super::get().no_result.is_empty());
    }
}
