//! Web forums: discover and add one, log in with the user's own account, and let the agent read
//! sections, topics and threads, search, and post. One backend today — `mobiquo`, the Tapatalk
//! plugin's XML-RPC — chosen per forum by its `kind`.
//!
//! CEILING: with a single backend the typed calls live in `mobiquo` and callers use them directly;
//! `store::add` keeps every stored forum `kind = "mobiquo"`. When a second kind arrives (Discourse's
//! JSON API, a plain-HTML reader), route the calls through a `match forum.kind` here and that stays
//! the one place that knows backend names.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

pub mod directory;
pub mod mobiquo;
pub mod store;
pub mod xmlrpc;

static DATA: LazyLock<Data> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/forums.toml"))
        .expect("data/forums.toml is checked by tests")
});

pub fn data() -> &'static Data {
    &DATA
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub directory: Directory,
    #[serde(default)]
    pub seed: Vec<Forum>,
    #[serde(default)]
    pub suggest: Vec<Suggestion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Directory {
    pub url: String,
    pub path: String,
    pub app_id: String,
    pub app_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub query: String,
}

/// A forum crowbot knows how to reach. A list of these is the store; every field past the endpoint
/// is optional so older files load. The password is never here — only the session cookies a login
/// returned.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Forum {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub mobiquo_dir: String,
    #[serde(default = "php")]
    pub ext: String,
    #[serde(default = "mobiquo_kind")]
    pub kind: String,
    #[serde(default)]
    pub user_agent: String,
    /// Empty means browsing as a guest.
    #[serde(default)]
    pub cookies: Vec<String>,
    /// Whom the stored cookies belong to, for display; `None` when browsing as a guest.
    #[serde(default)]
    pub username: Option<String>,
}

fn php() -> String {
    "php".to_owned()
}

fn mobiquo_kind() -> String {
    "mobiquo".to_owned()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub title: String,
    pub total: Option<i64>,
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
