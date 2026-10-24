//! Which models there are: the live list, else a recent cached one, else the list bundled with
//! this build.

pub mod table;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::api::Api;
use crate::api::models::{self, Listing, Model};
use crate::config::limits;
use crate::config::paths::Paths;
use crate::data;
use crate::io;

/// The list shipped in the binary, used when crowbot is unreachable and nothing is cached.
/// Refreshed by saving a verbatim `GET /v1/models` reply over data/models.json.
const SNAPSHOT: &str = data::raw!("models.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Live,
    Cache,
    Snapshot,
}

#[derive(Clone)]
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
struct Cached {
    fetched_at: Timestamp,
    data: Vec<Model>,
}

/// Fresh cache, else live, else stale cache, else the bundled snapshot: never fails.
pub async fn load(api: &Api, paths: &Paths, refresh: bool) -> Catalog {
    let path = paths.models_cache();
    let cached = read_cache(&path);
    let now = io::clock::now();
    let ttl = limits::get().models.cache_ttl_secs.signed_secs();

    if !refresh
        && let Some(cache) = &cached
        && io::clock::fresh(cache.fetched_at, now, ttl)
    {
        return from_cache(cache, None);
    }

    let note = match models::fetch(api).await {
        Ok(models) => {
            let cache = Cached {
                fetched_at: now,
                data: models,
            };
            // A cache that fails to write only costs a fetch next time.
            let _ = io::fs::write_json(&path, &cache, io::fs::Access::Shared);
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

/// An empty list, live or cached, is not "no models": the previous list is kept.
fn usable(models: &[Model]) -> bool {
    !models.is_empty()
}

fn read_cache(path: &std::path::Path) -> Option<Cached> {
    io::fs::read_json::<Cached>(path)
        .ok()
        .flatten()
        .filter(|cache| usable(&cache.data))
}

fn from_cache(cache: &Cached, note: Option<String>) -> Catalog {
    Catalog {
        models: cache.data.clone(),
        source: Source::Cache,
        fetched_at: Some(cache.fetched_at),
        note,
    }
}

/// The list bundled with this build, for when crowbot cannot be reached.
pub fn snapshot() -> Vec<Model> {
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
    fn an_empty_cache_counts_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let write = |data: Vec<Model>| {
            let cache = Cached {
                fetched_at: io::clock::now(),
                data,
            };
            io::fs::write_atomic(
                &path,
                &serde_json::to_vec(&cache).unwrap(),
                io::fs::Access::Shared,
            )
            .unwrap();
        };
        write(Vec::new());
        assert!(read_cache(&path).is_none());
        write(snapshot());
        assert!(read_cache(&path).is_some());
    }
}
