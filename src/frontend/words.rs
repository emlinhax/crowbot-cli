//! What both frontends say about a run, worded once (data/frontend.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::overflow;
use crate::conversation::message::Assistant;
use crate::data;
use crate::live::event::AgentEvent;
use crate::text::{template, units};

static WORDS: LazyLock<Words> = data::load!("frontend.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Words {
    pub interrupted: String,
    pub cut_off: String,
    pub window_full: String,
    pub unsaved: String,
    pub turn_limit: String,
    pub retrying: String,
    pub model_gone: String,
    pub mcp_failed: String,
    pub compact: CompactWords,
    pub print: PrintWords,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactWords {
    pub done: String,
    pub nothing: String,
    pub failed: String,
    pub auto: String,
    pub overflow: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrintWords {
    pub call: String,
    pub child_call: String,
    pub failed: String,
    pub needs_permission: String,
    pub unconfirmed: String,
    pub hint: String,
}

pub fn get() -> &'static Words {
    &WORDS
}

/// Why `reply` was cut off: its output limit, or a context window of `window` tokens that filled.
pub fn cut_off(reply: &Assistant, window: u64) -> &'static str {
    match overflow::window_filled(reply, window) {
        true => &WORDS.window_full,
        false => &WORDS.cut_off,
    }
}

/// What a finished summary of earlier turns comes to, and whether that is bad news; `why` is
/// what its start said (`auto`, `overflow`).
pub fn compaction(end: &AgentEvent, why: (bool, bool)) -> Option<(String, &'static str)> {
    let AgentEvent::CompactionEnd {
        summarised,
        kept,
        usage,
        error,
    } = end
    else {
        return None;
    };
    let words = &WORDS.compact;
    let why = match why {
        (_, true) => words.overflow.as_str(),
        (true, _) => words.auto.as_str(),
        _ => "",
    };
    Some(match (error, summarised) {
        (Some(error), _) => (
            template::fill(&words.failed, &[("why", why), ("error", error)]),
            "warn",
        ),
        (None, 0) => (words.nothing.clone(), "muted"),
        (None, count) => {
            let cost = units::usd_micros(usage.map_or(0, |u| u.cost_micros));
            let (count, kept) = (count.to_string(), kept.to_string());
            let vars = [
                ("why", why),
                ("count", &count),
                ("kept", &kept),
                ("cost", &cost),
            ];
            (template::fill(&words.done, &vars), "muted")
        }
    })
}

/// That MCP server `server` is not running, and why.
pub fn mcp_failed(server: &str, note: &str) -> String {
    template::fill(&WORDS.mcp_failed, &[("server", server), ("note", note)])
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
