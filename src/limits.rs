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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpLimits {
    pub connect_timeout_ms: Limit<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    pub fetch_timeout_ms: Limit<u64>,
    pub cache_ttl_secs: Limit<u64>,
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
