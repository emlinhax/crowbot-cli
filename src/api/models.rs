use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::api::Call;
use crate::app::App;
use crate::io;
use crate::limits;

/// The list shipped in the binary, used when crowbot is unreachable and nothing is cached.
const SNAPSHOT: &str = include_str!("../../data/models.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub pricing: Pricing,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// USD per million tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pricing {
    pub input_per_1m_usd: f64,
    #[serde(default)]
    pub cached_input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub streaming: bool,
    pub reasoning: bool,
    pub tool_calling: bool,
    pub temperature: bool,
    pub caching: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Live,
    Cache,
    Snapshot,
}

pub struct Catalog {
    pub models: Vec<Model>,
    pub source: Source,
    pub fetched_at: Option<Timestamp>,
    /// Why the live list was not used, when that is worth telling the user.
    pub note: Option<String>,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[derive(Serialize, Deserialize)]
struct Listing {
    data: Vec<Model>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: Timestamp,
    data: Vec<Model>,
}

/// Fresh cache, else live, else stale cache, else the bundled snapshot: never fails.
pub async fn load(app: &App, refresh: bool) -> Catalog {
    let path = app.paths.models_cache();
    let cached = io::fs::read_string(&path)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<Cached>(&text).ok());
    let now = io::clock::now();
    let ttl = SignedDuration::try_from(limits::get().models.cache_ttl_secs.secs())
        .unwrap_or(SignedDuration::MAX);

    if !refresh
        && let Some(cache) = &cached
        && is_fresh(cache.fetched_at, now, ttl)
    {
        return from_cache(cache, None);
    }

    let note = match fetch(app).await {
        Ok(models) => {
            let cache = Cached {
                fetched_at: now,
                data: models,
            };
            // A cache that fails to write only costs a fetch next time.
            if let Ok(json) = serde_json::to_vec_pretty(&cache) {
                let _ = io::fs::write_atomic(&path, &json, io::fs::Access::Shared);
            }
            return Catalog {
                models: cache.data,
                source: Source::Live,
                fetched_at: Some(now),
                note: None,
            };
        }
        Err(why) => why,
    };

    match &cached {
        Some(cache) => from_cache(cache, Some(note)),
        None => Catalog {
            models: snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: Some(note),
        },
    }
}

async fn fetch(app: &App) -> Result<Vec<Model>, String> {
    let timeout = limits::get().models.fetch_timeout_ms.ms();
    let resp = app
        .api
        .call("models", Call::default(), timeout)
        .await
        .map_err(|e| e.to_string())?;
    let listing: Listing =
        serde_json::from_slice(&resp.body).map_err(|e| format!("unreadable model list: {e}"))?;
    // An empty list must not read as "crowbot has no models"; keep what we had instead.
    if listing.data.is_empty() {
        return Err("crowbot returned an empty model list".into());
    }
    Ok(listing.data)
}

fn is_fresh(fetched_at: Timestamp, now: Timestamp, ttl: SignedDuration) -> bool {
    let age = now.duration_since(fetched_at);
    age >= SignedDuration::ZERO && age < ttl
}

fn from_cache(cache: &Cached, note: Option<String>) -> Catalog {
    Catalog {
        models: cache.data.clone(),
        source: Source::Cache,
        fetched_at: Some(cache.fetched_at),
        note,
    }
}

fn snapshot() -> Vec<Model> {
    serde_json::from_str::<Listing>(SNAPSHOT)
        .expect("data/models.json is checked by tests")
        .data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_parses_and_is_not_empty() {
        assert!(!snapshot().is_empty());
    }

    #[test]
    fn freshness_respects_ttl_and_clock_skew() {
        let t: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let ttl = SignedDuration::from_secs(60);
        assert!(is_fresh(t, t + SignedDuration::from_secs(59), ttl));
        assert!(!is_fresh(t, t + SignedDuration::from_secs(60), ttl));
        assert!(!is_fresh(t, t - SignedDuration::from_secs(1), ttl));
    }
}
