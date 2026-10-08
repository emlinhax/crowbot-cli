//! Web forums: discover, log in, read and post. A forum's `kind` names its backend; only `mobiquo`
//! (Tapatalk's XML-RPC) exists.
//!
//! CEILING: one backend, called directly (tools/forum/, commands/forums.rs). A second adds a
//! `match forum.kind` here and moves `mobiquo_dir`/`ext` under its kind.

use std::sync::LazyLock;

use anyhow::{Context, Result, bail};

use serde::{Deserialize, Serialize};

use crate::data;
use crate::io::fetch::{self, Fetch, Fetched};

pub mod account;
pub mod directory;
pub mod mobiquo;
pub mod store;
mod xmlrpc;

static DATA: LazyLock<Data> = data::load!("forums.toml");

pub fn data() -> &'static Data {
    &DATA
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub directory: directory::Directory,
    pub text: Text,
    #[serde(default)]
    pub seed: Vec<Forum>,
    #[serde(default)]
    pub suggest: Vec<Suggestion>,
}

/// What a failed call says when the forum gives no reason of its own.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub refused: String,
    pub rejected: String,
    pub login_failed: String,
    pub unreadable: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub query: String,
}

/// A POST to a forum or the directory, `who` naming it in errors; a refusal says so, and
/// whether a firewall gave it.
pub(crate) async fn post(fetch: &Fetch, who: &str, request: fetch::Post<'_>) -> Result<Fetched> {
    let resp = fetch
        .post(request)
        .await
        .with_context(|| format!("reaching {who}"))?;
    if resp.status >= 400 {
        let hint = if resp.firewall {
            " (a firewall refused it)"
        } else {
            ""
        };
        bail!("{who} replied HTTP {}{hint}", resp.status);
    }
    Ok(resp)
}

/// A stored forum. Fields past the endpoint are optional so older files load. Holds session
/// cookies, never the password.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Forum {
    pub id: String,
    pub name: String,
    /// What the forum covers, so the agent knows when to use it. From the seed or the directory.
    #[serde(default)]
    pub hint: String,
    pub base_url: String,
    #[serde(default)]
    pub mobiquo_dir: String,
    /// The plugin script's extension; empty leaves the backend's own.
    #[serde(default)]
    pub ext: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Empty means browsing as a guest.
    #[serde(default)]
    pub cookies: Vec<String>,
    /// Whom the stored cookies belong to, for display; `None` when browsing as a guest.
    #[serde(default)]
    pub username: Option<String>,
}

impl Forum {
    /// Its host, for display and permission rules; its lowercased name when the URL has none.
    pub fn host(&self) -> String {
        host(&self.base_url).unwrap_or_else(|| self.name.to_lowercase())
    }
}

/// The host of `url`, if it is a URL with one.
pub fn host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
}

fn default_kind() -> String {
    mobiquo::KIND.to_owned()
}

/// A section of a forum (a board), flattened from the tree with its depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub depth: usize,
    /// A heading that holds only sub-boards, with no topics of its own.
    pub sub_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub id: String,
    pub title: String,
    pub author: String,
    pub replies: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicList {
    pub topics: Vec<Topic>,
    pub total: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub author: String,
    pub time: String,
    pub content: String,
}

/// A page number, or the last page, where a thread's newest posts are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Num(i64),
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub title: String,
    pub total: Option<i64>,
    /// The page these posts are, and how many there are when the forum gives the total.
    pub page: i64,
    pub pages: Option<i64>,
    pub posts: Vec<Post>,
}

/// What a login settled on: whom we are, and the cookies to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub cookies: Vec<String>,
}

/// The outcome of a post, for the confirmation the user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub id: Option<String>,
    pub url: Option<String>,
}
