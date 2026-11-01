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
    let keys = spec.title.unwrap_or_default();
    // An empty list (`title = []`) is a deliberate no-title; a non-empty one that matched nothing
    // (an MCP tool titles by its own arg names) falls back to its values below.
    let wanted = !keys.is_empty();
    let present: Vec<String> = keys
        .into_iter()
        .filter_map(|key| arg_text(at(Some(&args), &key)?, items))
        .collect();
    let title = match spec.title_join {
        Some(sep) => (!present.is_empty()).then(|| present.join(&sep)),
        None => present.into_iter().next(),
    };
    let title = title.or_else(|| wanted.then(|| fallback(&args, items)).flatten())?;
    Some(title.lines().next().unwrap_or_default().to_owned())
}

/// When a tool's named title args are all absent — an MCP tool titles by its own argument names —
/// show its first few scalar argument values, so the card is not a bare tool name.
fn fallback(args: &Value, items: usize) -> Option<String> {
    let obj = args.as_object()?;
    let parts: Vec<String> = obj.values().filter_map(scalar).take(items.max(1)).collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let line = s.lines().next().unwrap_or_default();
            (!line.is_empty()).then(|| line.to_owned())
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The full value of a call's primary title arg when it spans more than one line — a heredoc or
/// piped script inside a bash `command` — so the card can show input the one-line header drops.
pub fn input(tool: &str, arguments: &str) -> Option<String> {
    let args: Value = serde_json::from_str(arguments).ok()?;
    let key = spec(tool)
        .title?
        .into_iter()
        .find(|k| at(Some(&args), k).is_some())?;
    let text = at(Some(&args), &key)?.as_str()?;
    (text.lines().count() > 1).then(|| text.to_owned())
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
            let parts: Vec<&str> = items.iter().filter_map(item_text).collect();
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

/// A batch item as text: a string, or an object's first string (read's `{path, offset, limit}`).
fn item_text(item: &Value) -> Option<&str> {
    match item {
        Value::Object(fields) => fields.values().find_map(Value::as_str),
        other => other.as_str(),
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
        let ranged = title("read", r#"{"path":[{"path":"a.rs","offset":9},"b.rs"]}"#);
        assert_eq!(ranged.as_deref(), Some("a.rs, b.rs"));
        // No args at all: still nothing to show.
        assert_eq!(title("frobnicate", "{}"), None);
    }

    #[test]
    fn an_unknown_tool_falls_back_to_its_arg_values() {
        // decompile's own section surfaces binary + function.
        let dec = title("decompile", r#"{"binary":"vault","function":"main"}"#);
        assert_eq!(dec.as_deref(), Some("vault main"));
        // An MCP tool titles by its own arg names, not the default keys: show the values.
        let mcp = title("ida_decompile", r#"{"address":"0x401000"}"#);
        assert_eq!(mcp.as_deref(), Some("0x401000"));
        // A deliberately title-less tool stays bare.
        assert_eq!(title("todowrite", r#"{"todos":[]}"#), None);
    }

    #[test]
    fn a_multi_line_command_is_offered_as_input_a_single_line_is_not() {
        let script = r#"{"command":"python3 - <<EOF\nprint(42)\nEOF"}"#;
        assert_eq!(
            input("bash", script).as_deref(),
            Some("python3 - <<EOF\nprint(42)\nEOF")
        );
        assert_eq!(input("bash", r#"{"command":"ls -la"}"#), None);
        assert_eq!(input("read", r#"{"path":"a.rs"}"#), None);
    }
}
