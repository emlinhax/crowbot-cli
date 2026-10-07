use std::num::NonZero;
use std::sync::LazyLock;
use std::time::Duration;

use jiff::SignedDuration;
use serde::Deserialize;

use crate::data;

static LIMITS: LazyLock<Limits> = data::load!("limits.toml");

pub fn get() -> &'static Limits {
    &LIMITS
}

/// A number and the reason it is that number; the reason is for readers of limits.toml.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit<T> {
    pub value: T,
    #[serde(rename = "reason")]
    _reason: String,
}

impl Limit<u64> {
    pub fn ms(&self) -> Duration {
        Duration::from_millis(self.value)
    }

    pub fn secs(&self) -> Duration {
        Duration::from_secs(self.value)
    }

    /// As a span for timestamp arithmetic; too large a value saturates instead of wrapping.
    pub fn signed_secs(&self) -> SignedDuration {
        SignedDuration::try_from(self.secs()).unwrap_or(SignedDuration::MAX)
    }

    pub fn signed_days(&self) -> SignedDuration {
        i64::try_from(self.value)
            .ok()
            .and_then(|days| days.checked_mul(86_400))
            .map_or(SignedDuration::MAX, SignedDuration::from_secs)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub http: HttpLimits,
    pub models: ModelLimits,
    pub chat: ChatLimits,
    pub retry: RetryLimits,
    pub pair: PairLimits,
    pub signup: SignupLimits,
    pub agent: AgentLimits,
    pub session: SessionLimits,
    pub tools: ToolLimits,
    pub update: UpdateLimits,
    pub forums: ForumLimits,
    pub edit: EditLimits,
    pub text: TextLimits,
    pub tui: TuiLimits,
    pub print: PrintLimits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpLimits {
    pub connect_timeout_ms: Limit<u64>,
    pub request_timeout_ms: Limit<u64>,
    pub error_body_ms: Limit<u64>,
    pub error_body_bytes: Limit<usize>,
    pub error_text_chars: Limit<usize>,
    pub fetch_timeout_secs: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    pub fetch_timeout_ms: Limit<u64>,
    pub cache_ttl_secs: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatLimits {
    pub headers_timeout_secs: Limit<u64>,
    pub idle_timeout_secs: Limit<u64>,
    pub max_event_bytes: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryLimits {
    pub max_attempts: Limit<u32>,
    pub initial_delay_ms: Limit<u64>,
    pub factor: Limit<u32>,
    pub max_delay_ms: Limit<u64>,
    pub jitter_pct: Limit<u64>,
    pub max_retry_after_secs: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairLimits {
    pub poll_interval_ms: Limit<u64>,
    pub backoff_ms: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignupLimits {
    pub max_attempts: Limit<u32>,
    pub max_bits: Limit<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLimits {
    pub max_turns: Limit<u32>,
    pub doom_loop_repeats: Limit<usize>,
    pub instructions_bytes: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolLimits {
    pub max_lines: Limit<usize>,
    pub max_bytes: Limit<usize>,
    pub max_line_chars: Limit<usize>,
    pub max_results: Limit<usize>,
    pub grep_context_max: Limit<usize>,
    pub grep_line_chars: Limit<usize>,
    pub read_batch_max: Limit<usize>,
    pub read_batch_bytes: Limit<usize>,
    pub list_depth_max: Limit<usize>,
    pub fs_batch_max: Limit<usize>,
    pub list_entries: Limit<usize>,
    pub question_max: Limit<usize>,
    pub question_options_max: Limit<usize>,
    pub binary_sniff_bytes: Limit<usize>,
    pub bash_timeout_secs: Limit<u64>,
    pub bash_max_timeout_secs: Limit<u64>,
    pub bash_drain_ms: Limit<u64>,
    pub bash_memory_bytes: Limit<usize>,
    pub bash_spill_max_bytes: Limit<usize>,
    pub bash_spill_keep_days: Limit<u64>,
    pub webfetch_max_bytes: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLimits {
    pub slug_chars: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLimits {
    pub check_interval_secs: Limit<u64>,
    pub download_timeout_secs: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForumLimits {
    pub page_size: Limit<NonZero<u32>>,
    pub batch_max: Limit<usize>,
    pub directory_results: Limit<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditLimits {
    pub anchor_similarity_pct: Limit<u64>,
    pub size_tolerance_pct: Limit<u64>,
    pub similarity_max_chars: Limit<usize>,
    pub hint_max_cells: Limit<usize>,
    pub hint_similarity_pct: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextLimits {
    pub rule_max: Limit<usize>,
    pub min_width: Limit<usize>,
    pub min_column: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiLimits {
    pub frame_ms: Limit<u64>,
    pub spinner_ms: Limit<u64>,
    pub paste_gap_ms: Limit<u64>,
    pub paste_min_keys: Limit<usize>,
    pub editor_max_rows_pct: Limit<usize>,
    pub reasoning_tail_lines: Limit<usize>,
    pub error_lines: Limit<usize>,
    pub quit_window_ms: Limit<u64>,
    pub history_max: Limit<usize>,
    pub prompt_body_lines: Limit<usize>,
    pub palette_rows: Limit<NonZero<usize>>,
    pub toast_ms: Limit<u64>,
    pub side_padding: Limit<usize>,
    pub scroll_lines: Limit<usize>,
    pub status_frame_ms: Limit<NonZero<u64>>,
    pub shimmer_ms: Limit<u64>,
    pub glint_rest: Limit<usize>,
    pub editor_min_rows: Limit<usize>,
    pub field_rows: Limit<usize>,
    pub welcome_gutter: Limit<usize>,
    pub welcome_min_text: Limit<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrintLimits {
    pub note_chars: Limit<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_parse() {
        let _ = get();
    }

    #[test]
    fn a_zero_divisor_is_refused_when_the_file_loads() {
        let src = data::raw!("limits.toml");
        let zero = src.replace(
            "[forums.page_size]\nvalue = 20",
            "[forums.page_size]\nvalue = 0",
        );
        assert_ne!(zero, src);
        assert!(toml::from_str::<Limits>(&zero).is_err());
    }

    #[test]
    fn signed_spans_saturate_instead_of_wrapping() {
        let limit = |value| Limit {
            value,
            _reason: String::new(),
        };
        assert_eq!(limit(2).signed_days(), SignedDuration::from_hours(48));
        assert_eq!(limit(90).signed_secs(), SignedDuration::from_secs(90));
        assert_eq!(limit(u64::MAX).signed_days(), SignedDuration::MAX);
        assert_eq!(limit(u64::MAX).signed_secs(), SignedDuration::MAX);
    }

    #[test]
    fn every_limit_states_a_reason() {
        fn walk(path: &str, value: &toml::Value) {
            let toml::Value::Table(table) = value else {
                return;
            };
            if table.contains_key("value") {
                let reason = table.get("reason").and_then(|r| r.as_str()).unwrap_or("");
                assert!(!reason.trim().is_empty(), "{path} has no reason");
                return;
            }
            for (key, child) in table {
                walk(&format!("{path}.{key}"), child);
            }
        }
        walk(
            "limits",
            &toml::from_str::<toml::Value>(data::raw!("limits.toml")).unwrap(),
        );
    }
}
