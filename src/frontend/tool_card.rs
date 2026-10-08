//! How a tool call is presented (data/tool_cards.toml), shared by both frontends: each tool's
//! spec, and the few words that title a call.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

use crate::data;
use crate::text::units::Plural;

static CARDS: LazyLock<BTreeMap<String, Spec>> = data::load!("tool_cards.toml");

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub title: Option<Vec<String>>,
    /// When set, the header joins every present title arg with this, instead of showing the first.
    #[serde(default)]
    pub title_join: Option<String>,
    #[serde(default)]
    pub body: Option<Body>,
    #[serde(default)]
    pub summary: Option<Summary>,
    #[serde(default)]
    pub max_lines: Option<usize>,
    /// A batch title arg shows this many items; past it, one fewer and `+N`.
    #[serde(default)]
    pub title_items: Option<usize>,
}

/// A line from the result's details, or a count of one of them in the form that fits it.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Summary {
    Text(String),
    Count {
        /// The details key holding the number; `{n}` in the forms.
        count: String,
        #[serde(flatten)]
        forms: Plural,
    },
    /// The first of these whose details are all there: one file's lines, else how many files.
    First(Vec<Summary>),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Body {
    None,
    Diff,
    Head,
    Tail,
    Summary,
}

/// A tool's section merged over `[default]`.
pub fn spec(tool: &str) -> Spec {
    let default = CARDS.get("default").cloned().unwrap_or_default();
    let own = CARDS.get(tool).cloned().unwrap_or_default();
    Spec {
        title: own.title.or(default.title),
        title_join: own.title_join.or(default.title_join),
        body: own.body.or(default.body),
        summary: own.summary.or(default.summary),
        max_lines: own.max_lines.or(default.max_lines),
        title_items: own.title_items.or(default.title_items),
    }
}

/// What a call is about in one short line (its path, command, forum…), from the args its spec
/// names; `None` when none is given.
pub fn title(tool: &str, arguments: &str) -> Option<String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let spec = spec(tool);
    let items = spec.title_items.unwrap_or_default();
    let present: Vec<String> = spec
        .title
        .unwrap_or_default()
        .into_iter()
        .filter_map(|key| arg_text(at(Some(&args), &key)?, items))
        .collect();
    // With `title_join` several present args fold onto one line (a forum and its section or search
    // term); otherwise the first present one titles the call, as every other tool does.
    let title = match spec.title_join {
        Some(sep) => (!present.is_empty()).then(|| present.join(&sep)),
        None => present.into_iter().next(),
    }?;
    Some(title.lines().next().unwrap_or_default().to_owned())
}

/// The `[default]` section every tool falls back to.
#[cfg(test)]
pub fn default_spec() -> &'static Spec {
    &CARDS["default"]
}

/// The tools that have a section of their own.
#[cfg(test)]
pub fn tools() -> impl Iterator<Item = &'static String> {
    CARDS.keys().filter(|k| *k != "default")
}

/// A title arg as one short line: a string's first line, or a batch array as its items (a few, then
/// `+N`); `None` when there is nothing to show.
fn arg_text(value: &Value, max_items: usize) -> Option<String> {
    match value {
        Value::String(s) => {
            let line = s.lines().next().unwrap_or_default();
            (!line.is_empty()).then(|| line.to_owned())
        }
        Value::Array(items) => {
            let parts: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            let shown = max_items.saturating_sub(1);
            match parts.len() {
                0 => None,
                n if n <= max_items => Some(parts.join(", ")),
                n => Some(format!("{}, +{}", parts[..shown].join(", "), n - shown)),
            }
        }
        _ => None,
    }
}

/// The value at a dotted path, `a.0.b` (keys, and indices into arrays), when there is one.
pub fn at<'v>(value: Option<&'v Value>, path: &str) -> Option<&'v Value> {
    value?.pointer(&format!("/{}", path.replace('.', "/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_titled_from_its_spec_in_either_frontend() {
        // Headless used to read only the default title args: no forum, no batch.
        let forum = title("forum_search", r#"{"forum":"uc","query":"aimbot"}"#);
        assert_eq!(forum.as_deref(), Some("uc · aimbot"));
        let batch = title("read", r#"{"path":["a.rs","b.rs"]}"#);
        assert_eq!(batch.as_deref(), Some("a.rs, b.rs"));
        assert_eq!(title("frobnicate", "{}"), None);
    }
}
