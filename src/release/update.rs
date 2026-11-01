//! Self-update to the newest release: `crowbot update`, or daily beside an interactive session
//! (release builds). `~/.crowbot/update.json` keeps the result for the next start.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, anyhow, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::api::endpoints;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::data;
use crate::io::http::{Http, Method, Request};
use crate::io::{self, fs::Access};
use crate::release::{self, Signing, signature};
use crate::text::digest;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = data::load!("update.toml", "text");

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
    unsigned: String,
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
struct State {
    #[serde(default)]
    pub checked_at: Option<Timestamp>,
    /// An update not yet announced: the next start says so, then clears it.
    #[serde(default)]
    pub updated: Option<Change>,
    #[serde(default)]
    /// Read by people, not code: the only trace of a failed background look.
    pub last_error: Option<String>,
}

fn load(paths: &Paths) -> State {
    io::fs::read_json(&paths.update_state())
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn save(paths: &Paths, state: &State) {
    // Losing the state costs one early check or one missed notice, nothing more.
    let _ = io::fs::write_json(&paths.update_state(), state, Access::Shared);
}

/// The update an earlier run installed, once: reading it clears it.
fn take_news(paths: &Paths) -> Option<Change> {
    let mut state = load(paths);
    let news = state.updated.take()?;
    save(paths, &state);
    Some(news)
}

/// Due: updates on, a release build, and `update.check_interval_secs` since the last look.
fn due(state: &State, now: Timestamp, enabled: bool) -> bool {
    let interval = limits::get().update.check_interval_secs.signed_secs();
    enabled
        && release::tag().is_some()
        && state
            .checked_at
            .is_none_or(|at| !io::clock::fresh(at, now, interval))
}

/// Who tells the user about an update `now` installs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tell {
    /// The caller, as it finishes; any news still pending is old by then and is cleared.
    Here,
    /// The next start, from the news this run keeps.
    NextStart,
}

/// What a session does about updates as it starts: clears away a replaced binary, hands back the
/// news an earlier run left (once), and when a look is due takes it in the background, recording
/// only what it did.
pub fn on_start(http: &Http, paths: &Paths, enabled: bool) -> Option<Change> {
    let _ = swept();
    let news = take_news(paths);
    if due(&load(paths), io::clock::now(), enabled) {
        let (http, paths) = (http.clone(), paths.clone());
        tokio::spawn(async move {
            let _ = now(&http, &paths, Tell::NextStart).await;
        });
    }
    news
}

/// This binary, with any copy an earlier update left beside it cleared away.
fn swept() -> anyhow::Result<std::path::PathBuf> {
    let exe = io::exe::current()?;
    io::exe::sweep(&exe);
    Ok(exe)
}

/// Checks and updates this binary now, recording the look and its result.
pub async fn now(http: &Http, paths: &Paths, tell: Tell) -> anyhow::Result<Outcome> {
    let current = release::tag().ok_or_else(|| anyhow!(TEXT.development.clone()))?;
    let exe = swept()?;
    let latest = endpoints::url(endpoints::get("latest_release"), &[]);
    let mut state = load(paths);
    state.checked_at = Some(io::clock::now());
    let result = run(http, &latest, current, &exe, release::signing()).await;
    match (&result, tell) {
        (Ok(_), Tell::Here) => {
            state.updated = None;
            state.last_error = None;
        }
        (Ok(outcome), Tell::NextStart) => {
            if let Outcome::Updated(change) = outcome {
                state.updated = Some(change.clone());
            }
            state.last_error = None;
        }
        (Err(e), _) => state.last_error = Some(format!("{e:#}")),
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

/// Replaces `exe` (release `current`) with the release at `latest_url` once its checksums carry a
/// signature by a key `signing` names and the download matches them.
async fn run(
    http: &Http,
    latest_url: &str,
    current: &str,
    exe: &Path,
    signing: &Signing,
) -> anyhow::Result<Outcome> {
    let listing = get(http, latest_url, limits::get().http.request_timeout_ms.ms()).await;
    let latest: Latest = listing
        .and_then(|body| Ok(serde_json::from_slice(&body)?))
        .map_err(|e| anyhow!(fill(&TEXT.listing, &[("error", &format!("{e:#}"))])))?;
    if latest.tag_name == current {
        return Ok(Outcome::UpToDate(current.to_owned()));
    }
    let ours = release::asset().ok_or_else(|| {
        let (os, arch) = release::platform();
        anyhow!(fill(&TEXT.no_platform, &[("os", os), ("arch", arch)]))
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
    let (file, sums, signed) = (
        find(&ours.file)?,
        find(release::sums())?,
        find(&signing.file)?,
    );
    let dir = exe.parent().context("the program has no folder")?;
    if !io::exe::writable(dir) {
        bail!(fill(
            &TEXT.not_writable,
            &[("dir", &dir.display().to_string())]
        ));
    }
    let download = limits::get().update.download_timeout_secs.secs();
    let bytes = get(http, &file.browser_download_url, download).await?;
    let sums = get(http, &sums.browser_download_url, download).await?;
    let signed = get(http, &signed.browser_download_url, download).await?;
    let tag = &latest.tag_name;
    if !signature::check(&sums, &String::from_utf8_lossy(&signed), tag, signing) {
        bail!(fill(
            &TEXT.unsigned,
            &[("file", release::sums()), ("tag", tag)]
        ));
    }
    if !matches(&bytes, &String::from_utf8_lossy(&sums), &ours.file) {
        bail!(fill(&TEXT.mismatch, &[("file", &ours.file)]));
    }
    io::exe::stage_and_replace(&bytes, exe)?;
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
    want.is_some_and(|want| want == digest::sha256_hex(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::routing::get;
    use jiff::SignedDuration;
    use serde_json::json;

    use super::*;
    use crate::release::signature::TestKey;

    fn sums_for(bytes: &[u8], name: &str) -> String {
        format!("{}  {name}\n", digest::sha256_hex(bytes))
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

    /// A stand-in for GitHub: the newest release `tag`, its file for this platform, its sums
    /// and, when there is one, their signature.
    async fn github(tag: &str, file: Vec<u8>, sums: String, signed: Option<String>) -> String {
        let name = release::asset().unwrap().file.clone();
        let file = Arc::new(file);
        let base = crate::testing::serve(|base| {
            let mut assets = vec![
                json!({"name": name, "browser_download_url": format!("{base}/file")}),
                json!({"name": release::sums(), "browser_download_url": format!("{base}/sums")}),
            ];
            if signed.is_some() {
                let url = format!("{base}/signed");
                assets.push(json!({"name": release::signing().file, "browser_download_url": url}));
            }
            let latest = json!({"tag_name": tag, "assets": assets});
            let signed = signed.unwrap_or_default();
            Router::new()
                .route("/latest", get(move || async move { axum::Json(latest) }))
                .route("/file", get(move || async move { file.as_ref().clone() }))
                .route("/sums", get(move || async move { sums }))
                .route("/signed", get(move || async move { signed }))
        })
        .await;
        format!("{base}/latest")
    }

    /// A release `tag` of `file`, its sums signed by `key` as that release.
    async fn signed_release(key: &TestKey, tag: &str, file: &[u8], sums_of: &[u8]) -> String {
        let sums = sums_for(sums_of, &release::asset().unwrap().file);
        let signed = key.sign(&sums, tag);
        github(tag, file.to_vec(), sums, Some(signed)).await
    }

    #[tokio::test]
    async fn a_newer_release_replaces_the_program_once_signed_and_its_checksum_matches() {
        if release::asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("crowbot");
        io::exe::write(&exe, b"old build").unwrap();
        let http = Http::new(std::time::Duration::from_secs(5)).unwrap();

        let key = TestKey::new();
        let run = |url: String, current: &'static str| {
            let (http, exe, signing) = (&http, &exe, &key.signing);
            async move { run(http, &url, current, exe, signing).await }
        };
        let url = signed_release(&key, "v0.2.0-2", b"new build", b"new build").await;
        let outcome = run(url.clone(), "v0.2.0-1").await.unwrap();
        assert_eq!(
            outcome,
            Outcome::Updated(Change {
                from: "v0.2.0-1".into(),
                to: "v0.2.0-2".into()
            })
        );
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
        assert_eq!(
            run(url, "v0.2.0-2").await.unwrap(),
            Outcome::UpToDate("v0.2.0-2".into())
        );

        let tampered = signed_release(&key, "v0.2.0-3", b"evil build", b"new build").await;
        let refused = run(tampered, "v0.2.0-2").await.unwrap_err();
        assert!(refused.to_string().contains("does not match"), "{refused}");
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
    }

    #[tokio::test]
    async fn a_release_whose_sums_are_not_signed_for_it_changes_nothing() {
        let Some(asset) = release::asset() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("crowbot");
        io::exe::write(&exe, b"old build").unwrap();
        let http = Http::new(std::time::Duration::from_secs(5)).unwrap();
        let key = TestKey::new();
        let sums = sums_for(b"evil build", &asset.file);
        let releases = [
            github("v0.2.0-2", b"evil build".to_vec(), sums.clone(), None).await,
            signed_release(&TestKey::new(), "v0.2.0-2", b"evil build", b"evil build").await,
            // An older release's signed sums, offered as a newer release.
            github(
                "v0.2.0-2",
                b"evil build".to_vec(),
                sums.clone(),
                Some(key.sign(&sums, "v0.2.0-1")),
            )
            .await,
        ];
        for url in releases {
            let refused = run(&http, &url, "v0.2.0-1", &exe, &key.signing)
                .await
                .unwrap_err();
            let said = refused.to_string();
            assert!(said.contains("SHA256SUMS"), "{said}");
            assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"old build");
        }
    }
}
