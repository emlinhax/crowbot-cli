//! An agent file: a kind of subagent the user writes, in Claude Code's shape. A `---` header of
//! `key: value` lines, then the prompt:
//!
//! ```text
//! ---
//! name: reviewer
//! description: Reviews a diff for bugs and reports them by file and line.
//! tools: Read, Grep, Glob, Bash
//! model: kimi-k3
//! ---
//! You are a careful reviewer...
//! ```
//!
//! `when` says what jobs to pick it for, beside `description`, for the agent choosing a kind.
//! `tools` left out gives every tool, as in Claude Code; `model: inherit` is the same as none.
//! crowbot adds `effort`, `except` (tools a full set leaves out) and `base` (`append` the prompt to
//! the session's, the default, or `replace` it, keeping only the environment block).
// CEILING: the header is `key: value` lines, not YAML; a list is `a, b` or `[a, b]`, and a
// multi-line value is refused. Upgrade: a YAML parser, once a key needs nesting.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::kinds::{Base, Kind};
use crate::data;
use crate::effort;
use crate::text::template::fill;

static DATA: LazyLock<Data> = data::load!("agent_files.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    extension: String,
    aliases: BTreeMap<String, String>,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    no_header: String,
    unclosed: String,
    bad_line: String,
    missing: String,
    unknown_key: String,
    bad_base: String,
    bad_effort: String,
    bad_name: String,
    empty: String,
    unknown_tool: String,
}

/// The keys a header may have. `color` is Claude Code's, read and left unused.
const KEYS: &[&str] = &[
    "name",
    "description",
    "when",
    "tools",
    "except",
    "model",
    "effort",
    "base",
    "color",
];

const FENCE: &str = "---";

/// The extension an agent file has.
pub fn extension() -> &'static str {
    &DATA.extension
}

/// The kind `text` describes, and notes on what in it was left out. `tools` are the names a kind
/// may give its subagent; an MCP tool (`server__tool`) is taken on trust, since servers start
/// later. `file` names it in what is said.
pub fn parse(text: &str, file: &str, tools: &[&str]) -> Result<(Kind, Vec<String>), String> {
    let words = &DATA.text;
    let said = |template: &str, more: &[(&str, &str)]| {
        let mut fields = vec![("file", file)];
        fields.extend_from_slice(more);
        fill(template, &fields)
    };
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some(FENCE) {
        return Err(said(&words.no_header, &[]));
    }
    let mut header = BTreeMap::new();
    let mut closed = false;
    for line in lines.by_ref() {
        let line = line.trim_end();
        if line == FENCE {
            closed = true;
            break;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| said(&words.bad_line, &[("line", line)]))?;
        let key = key.trim();
        if !KEYS.contains(&key) {
            let keys = KEYS.join(", ");
            return Err(said(&words.unknown_key, &[("key", key), ("keys", &keys)]));
        }
        header.insert(key.to_owned(), unquote(value.trim()).to_owned());
    }
    if !closed {
        return Err(said(&words.unclosed, &[]));
    }
    let prompt = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
    if prompt.is_empty() {
        return Err(said(&words.empty, &[]));
    }
    let field = |key: &str| header.get(key).filter(|v| !v.is_empty()).cloned();
    let need = |key: &str| field(key).ok_or_else(|| said(&words.missing, &[("key", key)]));
    let id = need("name")?;
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(said(&words.bad_name, &[("value", &id)]));
    }
    let summary = need("description")?;
    let base = match field("base").as_deref() {
        None | Some("append") => Base::Append,
        Some("replace") => Base::Replace,
        Some(other) => return Err(said(&words.bad_base, &[("value", other)])),
    };
    let effort = field("effort");
    if let Some(level) = &effort
        && effort::get(level).is_none()
    {
        return Err(said(&words.bad_effort, &[("value", level)]));
    }
    let model = field("model").filter(|m| m != "inherit");

    let mut notes = Vec::new();
    let mut known = |list: Option<String>| -> Vec<String> {
        let names = list.as_deref().map(names).unwrap_or_default();
        names
            .into_iter()
            .filter(|name| {
                let ok = name == "*" || name.contains("__") || tools.contains(&name.as_str());
                if !ok {
                    notes.push(said(&words.unknown_tool, &[("tool", name)]));
                }
                ok
            })
            .collect()
    };
    let named = known(field("tools"));
    let except = known(field("except"));
    let kind = Kind {
        id,
        summary,
        when: field("when"),
        tools: match named.is_empty() && field("tools").is_none() {
            true => vec!["*".to_owned()],
            false => named,
        },
        except,
        prompt,
        model,
        effort,
        base,
    };
    Ok((kind, notes))
}

/// A list's tool names, as crowbot calls them: `Read, Bash` or `[read, bash]`.
fn names(list: &str) -> Vec<String> {
    list.trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|name| unquote(name.trim()).to_lowercase())
        .filter(|name| !name.is_empty())
        .map(|name| DATA.aliases.get(&name).cloned().unwrap_or(name))
        .collect()
}

fn unquote(value: &str) -> &str {
    let quoted = |q: char| value.len() >= 2 && value.starts_with(q) && value.ends_with(q);
    match quoted('"') || quoted('\'') {
        true => &value[1..value.len() - 1],
        false => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLS: &[&str] = &["read", "grep", "glob", "bash", "edit", "write", "webfetch"];

    #[test]
    fn a_claude_code_agent_file_reads_as_a_kind() {
        let text = "---\nname: reviewer\ndescription: \"Reviews diffs.\"\nwhen: before calling a change done\ntools: Read, Grep, MultiEdit, mcp_x__q\nmodel: kimi-k3\ncolor: red\n---\n\nYou review.\n";
        let (kind, notes) = parse(text, "reviewer.md", TOOLS).unwrap();
        assert_eq!(kind.id, "reviewer");
        assert_eq!(kind.summary, "Reviews diffs.");
        assert_eq!(kind.when.as_deref(), Some("before calling a change done"));
        assert_eq!(kind.tools, ["read", "grep", "edit", "mcp_x__q"]);
        assert_eq!(kind.model.as_deref(), Some("kimi-k3"));
        assert_eq!(kind.base, Base::Append);
        assert_eq!(kind.prompt, "You review.");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn no_tools_means_every_tool_and_inherit_means_no_model() {
        let text = "---\nname: helper\ndescription: Helps.\nmodel: inherit\nbase: replace\nexcept: [bash]\n---\nHelp.";
        let (kind, _) = parse(text, "helper.md", TOOLS).unwrap();
        assert_eq!(kind.tools, ["*"]);
        assert_eq!(kind.except, ["bash"]);
        assert!(kind.model.is_none());
        assert_eq!(kind.base, Base::Replace);
    }

    #[test]
    fn an_unknown_tool_is_left_out_with_a_note() {
        let text = "---\nname: a\ndescription: b\ntools: Read, NotebookRead\n---\np";
        let (kind, notes) = parse(text, "a.md", TOOLS).unwrap();
        assert_eq!(kind.tools, ["read"]);
        assert!(notes[0].contains("notebookread"), "{notes:?}");
    }

    #[test]
    fn a_broken_file_is_refused_naming_itself_and_what_is_wrong() {
        for (text, says) in [
            ("no header", "no `---` header"),
            ("---\nname: a\n", "never closed"),
            ("---\nname: a\n---\np", "needs `description`"),
            (
                "---\nname: a\ndescription: b\nwat: c\n---\np",
                "unknown header key `wat`",
            ),
            (
                "---\nname: a b\ndescription: b\n---\np",
                "`name` is letters",
            ),
            (
                "---\nname: a\ndescription: b\neffort: max9\n---\np",
                "unknown effort",
            ),
            (
                "---\nname: a\ndescription: b\nbase: swap\n---\np",
                "`base` is",
            ),
            ("---\nname: a\ndescription: b\n---\n  \n", "no prompt"),
            ("---\nname a\n---\np", "not `key: value`"),
        ] {
            let err = parse(text, "x.md", TOOLS).err().unwrap();
            assert!(
                err.contains(says) && err.starts_with("x.md"),
                "{text:?}: {err}"
            );
        }
    }

    /// docs/agents/, the examples the README points to.
    const EXAMPLES: &[(&str, &str)] = &[
        (
            "debugger",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/debugger.md"
            )),
        ),
        (
            "planner",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/planner.md"
            )),
        ),
        (
            "researcher",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/researcher.md"
            )),
        ),
        (
            "reverser",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/reverser.md"
            )),
        ),
        (
            "reviewer",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/reviewer.md"
            )),
        ),
        (
            "tester",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/docs/agents/tester.md"
            )),
        ),
    ];

    #[test]
    fn the_documented_examples_parse_and_name_only_real_tools() {
        let registry = crate::tools::Registry::builtin(None);
        let tools: Vec<&str> = registry.each_name().collect();
        for (name, text) in EXAMPLES {
            let (kind, notes) = parse(text, name, &tools).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(kind.id, *name);
            assert!(notes.is_empty(), "{name}: {notes:?}");
            assert!(kind.when.is_some() && kind.effort.is_some(), "{name}");
        }
    }
}
