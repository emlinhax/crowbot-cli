//! crowbot replaces itself with the newest release: on demand (`crowbot update`), and once a day
//! in the background of an interactive session for a release build. What happened is kept in
//! `~/.crowbot/update.json`, so the next start can say so.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, anyhow, bail};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::endpoints;
use crate::io::http::{Http, Method, Request};
use crate::io::{self, fs::Access};
use crate::limits;
use crate::paths::Paths;
use crate::release;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(include_str!("../data/update.toml"))
        .expect("data/update.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    updated: String,
    up_to_date: String,
    development: String,
    listing: String,
    no_platform: String,
    missing: String,
    not_writable: String,
    mismatch: String,
    http: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Updated(Change),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub from: String,
    pub to: String,
}

impl Outcome {
    pub fn say(&self) -> String {
        match self {
            Self::UpToDate(tag) => fill(&TEXT.up_to_date, &[("tag", tag)]),
            Self::Updated(change) => change.say(),
        }
    }
}

impl Change {
    pub fn say(&self) -> String {
        fill(&TEXT.updated, &[("from", &self.from), ("to", &self.to)])
    }
}

/// What the updater remembers between runs. Every field is optional, so older files load.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub checked_at: Option<Timestamp>,
    /// An update not yet announced: the next start says so, then clears it.
    #[serde(default)]
    pub updated: Option<Change>,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub fn load(paths: &Paths) -> State {
    io::fs::read_string(&paths.update_state())
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(paths: &Paths, state: &State) {
    // Losing the state costs one early check or one missed notice, nothing more.
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let _ = io::fs::write_atomic(&paths.update_state(), &json, Access::Shared);
    }
}

/// The update an earlier run installed, once: reading it clears it.
pub fn take_news(paths: &Paths) -> Option<Change> {
    let mut state = load(paths);
    let news = state.updated.take()?;
    save(paths, &state);
    Some(news)
}

/// Whether an interactive session should look now: updates on, a release build, and a day since
/// the last look.
pub fn due(state: &State, now: Timestamp, enabled: bool) -> bool {
    let interval = SignedDuration::try_from(limits::get().update.check_interval_secs.secs())
        .unwrap_or(SignedDuration::MAX);
    enabled
        && release::tag().is_some()
        && state
            .checked_at
            .is_none_or(|at| now.duration_since(at) >= interval)
}

/// Checks and updates this binary now, recording the look and its result.
pub async fn now(paths: &Paths) -> anyhow::Result<Outcome> {
    let current = release::tag().ok_or_else(|| anyhow!(TEXT.development.clone()))?;
    let exe = io::proc::current_exe()?;
    io::fs::sweep_replaced(&exe);
    let http = Http::new(limits::get().http.connect_timeout_ms.ms())?;
    let latest = endpoints::url(endpoints::get("latest_release"), &[]);
    let mut state = load(paths);
    state.checked_at = Some(io::clock::now());
    let result = run(&http, &latest, current, &exe).await;
    match &result {
        Ok(Outcome::Updated(change)) => {
            state.updated = Some(change.clone());
            state.last_error = None;
        }
        Ok(Outcome::UpToDate(_)) => state.last_error = None,
        Err(e) => state.last_error = Some(format!("{e:#}")),
    }
    save(paths, &state);
    result
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    assets: Vec<Remote>,
}

#[derive(Deserialize)]
struct Remote {
    name: String,
    browser_download_url: String,
}

/// Replaces `exe`, built as release `current`, with the release `latest_url` names, after
/// checking the download against the release's checksums.
/// CEILING: the checksums come from the same release, so they catch a damaged download, not a
/// hostile upload. Upgrade: a minisign signature checked against a key built into crowbot.
async fn run(http: &Http, latest_url: &str, current: &str, exe: &Path) -> anyhow::Result<Outcome> {
    let listing = get(http, latest_url, limits::get().http.request_timeout_ms.ms()).await;
    let latest: Latest = listing
        .and_then(|body| Ok(serde_json::from_slice(&body)?))
        .map_err(|e| anyhow!(fill(&TEXT.listing, &[("error", &format!("{e:#}"))])))?;
    if latest.tag_name == current {
        return Ok(Outcome::UpToDate(current.to_owned()));
    }
    let ours = release::asset().ok_or_else(|| {
        anyhow!(fill(
            &TEXT.no_platform,
            &[
                ("os", std::env::consts::OS),
                ("arch", std::env::consts::ARCH)
            ]
        ))
    })?;
    let find = |name: &str| {
        latest
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| {
                anyhow!(fill(
                    &TEXT.missing,
                    &[("tag", &latest.tag_name), ("file", name)]
                ))
            })
    };
    let (file, sums) = (find(&ours.file)?, find(release::sums())?);
    let dir = exe.parent().context("the program has no folder")?;
    if !io::fs::writable(dir) {
        bail!(fill(
            &TEXT.not_writable,
            &[("dir", &dir.display().to_string())]
        ));
    }
    let download = limits::get().update.download_timeout_secs.secs();
    let bytes = get(http, &file.browser_download_url, download).await?;
    let sums = get(http, &sums.browser_download_url, download).await?;
    if !matches(&bytes, &String::from_utf8_lossy(&sums), &ours.file) {
        bail!(fill(&TEXT.mismatch, &[("file", &ours.file)]));
    }
    let name = exe
        .file_name()
        .map_or_else(|| "crowbot".into(), |n| n.to_string_lossy());
    let staged = dir.join(format!(".{name}.new"));
    io::fs::write_executable(&staged, &bytes)?;
    if let Err(e) = io::fs::replace_executable(&staged, exe) {
        let _ = io::fs::remove(&staged);
        return Err(e.into());
    }
    Ok(Outcome::Updated(Change {
        from: current.to_owned(),
        to: latest.tag_name,
    }))
}

async fn get(http: &Http, url: &str, timeout: std::time::Duration) -> anyhow::Result<Vec<u8>> {
    let resp = http
        .send(Request {
            method: Method::Get,
            url: url.to_owned(),
            bearer: None,
            headers: &[("Accept", "application/vnd.github+json")],
            json: None,
            timeout,
        })
        .await?;
    if resp.head.status >= 400 {
        bail!(fill(
            &TEXT.http,
            &[("url", url), ("status", &resp.head.status.to_string())]
        ));
    }
    Ok(resp.body)
}

/// Whether `bytes` hash to what `sums` (sha256sum's `<hex>  <name>` lines) says `name` is.
fn matches(bytes: &[u8], sums: &str, name: &str) -> bool {
    let want = sums.lines().find_map(|line| {
        let (hex, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then(|| hex.to_lowercase())
    });
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    want.is_some_and(|want| want == got)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::routing::get;
    use serde_json::json;

    use super::*;

    fn sums_for(bytes: &[u8], name: &str) -> String {
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{hex}  {name}\n")
    }

    #[test]
    fn a_download_matches_only_its_own_line() {
        let sums = format!("{}{}", sums_for(b"one", "a"), sums_for(b"two", "b"));
        assert!(matches(b"two", &sums, "b"));
        assert!(!matches(b"one", &sums, "b"));
        assert!(!matches(b"two", &sums, "c"));
    }

    #[test]
    fn a_look_is_due_once_a_day_for_a_release_build_with_updates_on() {
        let now: Timestamp = "2026-10-03T12:00:00Z".parse().unwrap();
        let recent = State {
            checked_at: Some(now - SignedDuration::from_hours(2)),
            ..State::default()
        };
        let stale = State {
            checked_at: Some(now - SignedDuration::from_hours(25)),
            ..State::default()
        };
        assert!(!due(&recent, now, true));
        assert!(!due(&stale, now, false));
        // A test binary is a development build, which never updates itself.
        assert_eq!(due(&stale, now, true), release::tag().is_some());
    }

    /// A stand-in for GitHub: the newest release `tag`, its file for this platform, and sums.
    async fn github(tag: &str, file: Vec<u8>, sums: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let name = release::asset().unwrap().file.clone();
        let latest = json!({"tag_name": tag, "assets": [
            {"name": name, "browser_download_url": format!("{base}/file")},
            {"name": release::sums(), "browser_download_url": format!("{base}/sums")},
        ]});
        let file = Arc::new(file);
        let router = Router::new()
            .route("/latest", get(move || async move { axum::Json(latest) }))
            .route("/file", get(move || async move { file.as_ref().clone() }))
            .route("/sums", get(move || async move { sums }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("{base}/latest")
    }

    #[tokio::test]
    async fn a_newer_release_replaces_the_program_once_its_checksum_matches() {
        let Some(asset) = release::asset() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("crowbot");
        io::fs::write_executable(&exe, b"old build").unwrap();
        let http = Http::new(std::time::Duration::from_secs(5)).unwrap();

        let url = github(
            "v0.2.0-2",
            b"new build".to_vec(),
            sums_for(b"new build", &asset.file),
        )
        .await;
        let outcome = run(&http, &url, "v0.2.0-1", &exe).await.unwrap();
        assert_eq!(
            outcome,
            Outcome::Updated(Change {
                from: "v0.2.0-1".into(),
                to: "v0.2.0-2".into()
            })
        );
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
        assert_eq!(
            run(&http, &url, "v0.2.0-2", &exe).await.unwrap(),
            Outcome::UpToDate("v0.2.0-2".into())
        );

        let tampered = github(
            "v0.2.0-3",
            b"evil build".to_vec(),
            sums_for(b"new build", &asset.file),
        )
        .await;
        let refused = run(&http, &tampered, "v0.2.0-2", &exe).await.unwrap_err();
        assert!(refused.to_string().contains("does not match"), "{refused}");
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
    }
}
