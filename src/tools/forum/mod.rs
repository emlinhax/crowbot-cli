//! The forum tools the model can call: browse sections, topics and threads, search, and post. All
//! read-only ones are allowed by default; the posting one shows the user the exact text and waits
//! for a yes every time, even in AUTO. Forum content is untrusted — the descriptions tell the model
//! so, and nothing here acts on it automatically.

mod browse;
mod post;
mod render;

use std::sync::LazyLock;

use serde::Deserialize;

pub use browse::{Search, Sections, ThreadTool, Topics};
pub use post::Post;

use super::ToolCx;
use crate::data;
use crate::forums::{Forum, store};
use crate::text::template::fill;

static TEXT: LazyLock<Text> = data::load!("tools/forum.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    no_forums: String,
    unknown: String,
    no_subject: String,
    posted: String,
    cut: String,
}

/// Find the forum `key` names, or an error the model can act on, from one read of the store.
fn find(cx: &ToolCx<'_>, key: &str) -> Result<Forum, String> {
    let list = store::load(&cx.app.paths).map_err(|e| format!("{e:#}"))?;
    if list.is_empty() {
        return Err(TEXT.no_forums.clone());
    }
    list.into_iter()
        .find(|f| store::matches(f, key))
        .ok_or_else(|| fill(&TEXT.unknown, &[("forum", key)]))
}
