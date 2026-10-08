//! What both frontends say about a run, worded once (data/frontend.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static WORDS: LazyLock<Words> = data::load!("frontend.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Words {
    pub interrupted: String,
    pub cut_off: String,
    pub unsaved: String,
    pub turn_limit: String,
    pub retrying: String,
    pub print: PrintWords,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrintWords {
    pub call: String,
    pub failed: String,
    pub needs_permission: String,
    pub hint: String,
}

pub fn get() -> &'static Words {
    &WORDS
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        assert!(!super::get().print.call.is_empty());
    }
}
