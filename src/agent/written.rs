//! Tool calls a model wrote into its reply's text instead of making them, which some models do
//! now and then (data/written_calls.toml): the run would end on a reply that only says what it
//! meant to call.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static DATA: LazyLock<Data> = data::load!("written_calls.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    markers: Vec<String>,
}

/// Whether `text`, a reply that made no call, holds calls written out.
pub fn wrote_calls(text: &str) -> bool {
    DATA.markers.iter().any(|m| text.contains(m.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_written_out_are_told_from_prose() {
        assert!(wrote_calls(
            r#"I'll search. {"tool_calls":[{"name":"docs__search"}]}"#
        ));
        assert!(wrote_calls("<tool_call>{\"name\": \"read\"}</tool_call>"));
        assert!(!wrote_calls("The docs say alerts are retried 5 times."));
    }
}
