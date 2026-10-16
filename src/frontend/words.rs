//! What both frontends say about a run, worded once (data/frontend.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::text::template;

static WORDS: LazyLock<Words> = data::load!("frontend.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Words {
    pub interrupted: String,
    pub cut_off: String,
    pub unsaved: String,
    pub turn_limit: String,
    pub retrying: String,
    pub model_gone: String,
    pub print: PrintWords,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrintWords {
    pub call: String,
    pub failed: String,
    pub needs_permission: String,
    pub unconfirmed: String,
    pub hint: String,
}

pub fn get() -> &'static Words {
    &WORDS
}

/// That a resumed session's model is no longer served, and what it goes on with instead.
pub fn model_gone(model: &str, instead: &str) -> String {
    template::fill(&WORDS.model_gone, &[("model", model), ("instead", instead)])
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        assert!(!super::get().print.call.is_empty());
    }
}
