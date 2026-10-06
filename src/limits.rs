use std::sync::LazyLock;
use std::time::Duration;

use serde::Deserialize;

const SRC: &str = include_str!("../data/limits.toml");

static LIMITS: LazyLock<Limits> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/limits.toml is checked by tests"));

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
    pub tools: ToolLimits,
    pub update: UpdateLimits,
    pub forums: ForumLimits,
    pub edit: EditLimits,
    pub tui: TuiLimits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpLimits {
    pub connect_timeout_ms: Limit<u64>,
    pub request_timeout_ms: Limit<u64>,
    pub error_body_ms: Limit<u64>,
    pub error_body_bytes: Limit<usize>,
    pub error_text_chars: Limit<usize>,
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
    pub binary_sniff_bytes: Limit<usize>,
    pub bash_timeout_secs: Limit<u64>,
    pub bash_max_timeout_secs: Limit<u64>,
    pub bash_drain_ms: Limit<u64>,
    pub bash_memory_bytes: Limit<usize>,
    pub bash_spill_max_bytes: Limit<usize>,
    pub bash_spill_keep_days: Limit<u64>,
    pub webfetch_max_bytes: Limit<usize>,
    pub webfetch_timeout_secs: Limit<u64>,
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
    pub page_size: Limit<i64>,
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
    pub palette_rows: Limit<usize>,
    pub toast_ms: Limit<u64>,
    pub side_padding: Limit<usize>,
    pub scroll_lines: Limit<usize>,
    pub status_frame_ms: Limit<u64>,
    pub shimmer_ms: Limit<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_parse() {
        let _ = get();
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
        walk("limits", &toml::from_str::<toml::Value>(SRC).unwrap());
    }
}
