//! What a simulated tool's call does to the server's data, and what it answers.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use super::load::{Act, Format, PageKind, Tool};

pub type Item = Map<String, Value>;
pub type Collections = BTreeMap<String, Vec<Item>>;

/// How a call went, as the call log records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    InvalidArgs,
    NotFound,
    Precondition,
    Transient,
    UnknownTool,
}

/// A call that did not do what it asked, and what the server says.
pub struct Fail {
    pub outcome: Outcome,
    pub text: String,
}

impl Fail {
    pub fn new(outcome: Outcome, text: impl Into<String>) -> Self {
        Self {
            outcome,
            text: text.into(),
        }
    }
}

/// The answer, and whether the data changed.
pub fn act(tool: &Tool, args: &Item, data: &mut Collections) -> Result<(String, bool), Fail> {
    match tool.act {
        Act::Text => Ok((fill(text(tool), None, args), false)),
        Act::List => list(tool, args, data).map(|answer| (answer, false)),
        Act::Count => {
            let n = matching(tool, args, collection(tool, data))?.len();
            Ok((n.to_string(), false))
        }
        Act::Get => {
            let items = collection(tool, data);
            let at = find(tool, args, items)?;
            Ok((fill(render(tool), Some(&items[at]), args), false))
        }
        Act::Update => {
            let at = find(tool, args, collection(tool, data))?;
            let item = &mut collection_mut(tool, data)[at];
            for (field, arg) in &tool.set {
                if let Some(value) = args.get(arg) {
                    item.insert(field.clone(), value.clone());
                }
            }
            Ok((fill(text(tool), Some(item), args), true))
        }
        Act::Create => {
            let items = collection_mut(tool, data);
            let id = items
                .iter()
                .filter_map(|i| i.get("id").and_then(Value::as_u64))
                .max()
                .unwrap_or(0)
                + 1;
            let mut item = Item::new();
            item.insert("id".into(), Value::from(id));
            for (field, arg) in &tool.set {
                if let Some(value) = args.get(arg) {
                    item.insert(field.clone(), value.clone());
                }
            }
            let answer = fill(text(tool), Some(&item), args);
            items.push(item);
            Ok((answer, true))
        }
    }
}

fn text(tool: &Tool) -> &str {
    tool.text.as_deref().unwrap_or("Done.")
}

fn render(tool: &Tool) -> &str {
    tool.render.as_deref().unwrap_or("{id}")
}

fn collection<'a>(tool: &Tool, data: &'a Collections) -> &'a [Item] {
    let name = tool
        .collection
        .as_deref()
        .expect("this act names a collection");
    data.get(name).map(Vec::as_slice).unwrap_or_default()
}

fn collection_mut<'a>(tool: &Tool, data: &'a mut Collections) -> &'a mut Vec<Item> {
    let name = tool
        .collection
        .clone()
        .expect("this act names a collection");
    data.entry(name).or_default()
}

/// The items `matches`, `where_arg` and `filter` let through, in order.
fn matching<'a>(tool: &Tool, args: &Item, items: &'a [Item]) -> Result<Vec<&'a Item>, Fail> {
    let mut wanted: Vec<(String, Value)> = tool
        .matches
        .iter()
        .filter_map(|(field, arg)| Some((field.clone(), args.get(arg)?.clone())))
        .collect();
    if let Some(arg) = &tool.where_arg
        && let Some(Value::Object(conditions)) = args.get(arg)
    {
        let columns: Vec<&String> = items
            .first()
            .map(|i| i.keys().collect())
            .unwrap_or_default();
        for (field, value) in conditions {
            if !columns.contains(&field) {
                let known: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
                return Err(Fail::new(
                    Outcome::InvalidArgs,
                    format!("no column `{field}`; the columns are {}", known.join(", ")),
                ));
            }
            wanted.push((field.clone(), value.clone()));
        }
    }
    let needle = tool
        .filter
        .as_ref()
        .and_then(|f| args.get(&f.arg)?.as_str())
        .map(str::to_lowercase)
        .filter(|n| !n.is_empty());
    Ok(items
        .iter()
        .filter(|item| {
            wanted.iter().all(|(field, value)| match item.get(field) {
                Some(Value::Array(values)) => values.contains(value),
                Some(have) => have == value,
                None => false,
            })
        })
        .filter(|item| match (&needle, &tool.filter) {
            (Some(needle), Some(filter)) => filter.fields.iter().any(|f| {
                item.get(f)
                    .is_some_and(|v| shown(v).to_lowercase().contains(needle))
            }),
            _ => true,
        })
        .collect())
}

fn list(tool: &Tool, args: &Item, data: &Collections) -> Result<String, Fail> {
    let items = matching(tool, args, collection(tool, data))?;
    let total = items.len();
    let (start, size) = match &tool.page {
        None => (0, total.max(1)),
        Some(page) => {
            let start = match (page.kind, args.get(&page.arg)) {
                (_, None) => 0,
                (PageKind::Offset, Some(v)) => v.as_u64().unwrap_or(0) as usize,
                (PageKind::Cursor, Some(v)) => v
                    .as_str()
                    .and_then(|c| c.strip_prefix('c'))
                    .and_then(|n| n.parse().ok())
                    .ok_or_else(|| {
                        Fail::new(
                            Outcome::InvalidArgs,
                            format!(
                                "`{}`: unknown cursor {v}; pass the one the last page gave",
                                page.arg
                            ),
                        )
                    })?,
            };
            let limit = page
                .limit
                .as_ref()
                .and_then(|l| args.get(l)?.as_u64())
                .map_or(page.size, |l| (l as usize).clamp(1, page.size));
            (start, limit)
        }
    };
    if total == 0 {
        return Ok("Nothing matches.".into());
    }
    let end = (start + size).min(total);
    if start >= total {
        return Ok(format!("Nothing past {total} matches."));
    }
    let mut lines = vec![format!("{}-{end} of {total}:", start + 1)];
    lines.extend(
        items[start..end]
            .iter()
            .map(|i| fill(render(tool), Some(i), args)),
    );
    if let Some(page) = &tool.page
        && end < total
    {
        lines.push(match page.kind {
            PageKind::Offset => format!("More: call again with {}={end}.", page.arg),
            PageKind::Cursor => format!("More: call again with {}=\"c{end}\".", page.arg),
        });
    }
    Ok(lines.join("\n"))
}

/// The index of the item the arguments name by key, or why there is none.
fn find(tool: &Tool, args: &Item, items: &[Item]) -> Result<usize, Fail> {
    let key = tool.key.as_deref().expect("this act names a key");
    let arg = tool.arg.as_deref().unwrap_or(key);
    let Some(value) = args.get(arg) else {
        return Err(Fail::new(Outcome::InvalidArgs, format!("missing `{arg}`")));
    };
    let hex = tool.format == Some(Format::Hex);
    if hex && address(value).is_none() {
        return Err(Fail::new(
            Outcome::InvalidArgs,
            format!("`{arg}` must be a hex address like 0x401000, got {value}"),
        ));
    }
    let same = |have: &Value| match hex {
        true => address(have).is_some() && address(have) == address(value),
        false => have == value,
    };
    if let Some(at) = items.iter().position(|i| i.get(key).is_some_and(same)) {
        return Ok(at);
    }
    let missing = tool
        .missing
        .as_deref()
        .map(|m| fill(m, None, args))
        .unwrap_or_else(|| format!("No item with {key} {}.", shown(value)));
    let near = near(key, value, hex, items);
    let text = match near.is_empty() {
        true => missing,
        false => format!("{missing} Near: {}.", near.join(", ")),
    };
    Err(Fail::new(Outcome::NotFound, text))
}

/// Up to three keys close to `value`: nearest addresses, else keys that contain it or that it
/// contains, with each item's `name` when it has one.
fn near(key: &str, value: &Value, hex: bool, items: &[Item]) -> Vec<String> {
    let label = |i: &Item| match i.get("name") {
        Some(name) if key != "name" => format!("{} ({})", shown(&i[key]), shown(name)),
        _ => shown(&i[key]),
    };
    let mut close: Vec<&Item> = items.iter().filter(|i| i.contains_key(key)).collect();
    if hex {
        let want = address(value).unwrap_or(0);
        close.sort_by_key(|i| address(&i[key]).unwrap_or(0).abs_diff(want));
    } else {
        let want = shown(value).to_lowercase();
        close.retain(|i| {
            let have = shown(&i[key]).to_lowercase();
            have.contains(&want) || want.contains(&have)
        });
    }
    close.iter().take(3).map(|i| label(i)).collect()
}

fn address(value: &Value) -> Option<u64> {
    let text = value.as_str()?;
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    u64::from_str_radix(digits, 16).ok()
}

/// `{name}` filled from `item`, else from `args`; anything else left as written.
pub fn fill(template: &str, item: Option<&Item>, args: &Item) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let name = &rest[open + 1..open + close];
        out.push_str(&rest[..open]);
        match item.and_then(|i| i.get(name)).or_else(|| args.get(name)) {
            Some(value) => out.push_str(&shown(value)),
            None => out.push_str(&rest[open..=open + close]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// A value as an answer shows it: a string as written, a list joined.
pub fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(shown).collect::<Vec<_>>().join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
