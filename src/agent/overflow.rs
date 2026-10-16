//! When the conversation no longer fits the model's window: a request refused as too long
//! (data/overflow.toml), a reply cut off because the window was full, or a last request so big
//! the next one should be summarised first (`compaction.*` in data/limits.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::models::Model;
use crate::config::limits;
use crate::conversation::error::ErrorInfo;
use crate::conversation::message::{Assistant, Finish};
use crate::data;

static DATA: LazyLock<Data> = data::load!("overflow.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    says: Vec<String>,
    kind: Vec<Kind>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Kind {
    id: String,
    #[serde(default)]
    always: bool,
}

/// Whether crowbot or its vendor refused the request for being too long.
pub fn refused(error: &ErrorInfo) -> bool {
    let Some(kind) = DATA.kind.iter().find(|k| k.id == error.kind) else {
        return false;
    };
    let message = error.message.to_lowercase();
    kind.always
        || DATA
            .says
            .iter()
            .any(|s| message.contains(&s.to_lowercase()))
}

/// Whether `reply` was cut off because the window filled, not because its output ran out.
pub fn window_filled(reply: &Assistant, window: u64) -> bool {
    let Some(usage) = reply.usage else {
        return false;
    };
    let pct = limits::get().compaction.length_overflow_pct.value;
    // A window of 0 is one not yet known, never one that filled.
    window > 0
        && reply.finish == Finish::Length
        && (usage.input + usage.output) * 100 >= window * pct
}

/// Whether a request that sent `input` tokens leaves too little of `model`'s window: past
/// `compaction.threshold_pct` of what remains once the model has room to reply.
pub fn nearly_full(input: u64, model: &Model) -> bool {
    let room = model.context_window.saturating_sub(model.max_output_tokens);
    input * 100 > room * limits::get().compaction.threshold_pct.value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Usage;

    #[test]
    fn a_refusal_is_an_overflow_by_its_kind_and_words() {
        let error = |kind: &str, message: &str| ErrorInfo::local(kind, message);
        assert!(refused(&error(
            "upstream_error",
            "Prompt is too long: 210000 tokens"
        )));
        assert!(refused(&error(
            "bad_request",
            "maximum context length is 131072"
        )));
        assert!(refused(&error("body_too_large", "anything")));
        assert!(!refused(&error("upstream_error", "the vendor is down")));
        assert!(!refused(&error("rate_limited", "prompt is too long")));
    }

    #[test]
    fn a_cut_reply_filled_the_window_only_near_its_end() {
        let cut = |input, output| Assistant {
            usage: Some(Usage {
                input,
                output,
                ..Usage::default()
            }),
            ..Assistant::stub(Finish::Length, Vec::new())
        };
        assert!(window_filled(&cut(95_000, 4_000), 100_000));
        assert!(!window_filled(&cut(10_000, 4_000), 100_000));
        let done = Assistant {
            finish: Finish::Done,
            ..cut(99_000, 1_000)
        };
        assert!(!window_filled(&done, 100_000));
    }

    #[test]
    fn a_request_is_nearly_full_past_the_share_of_what_the_reply_leaves() {
        let model = crate::testing::model(false);
        // 100k window, 1k for the reply: 85% of 99k is 84,150.
        assert!(nearly_full(84_151, &model));
        assert!(!nearly_full(84_150, &model));
    }
}
