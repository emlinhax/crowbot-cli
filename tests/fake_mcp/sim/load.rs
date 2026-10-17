//! A simulated server as its file describes it (bench/mcp/*.toml): its words, its data and one
//! entry per tool saying what a call does.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// Names its record files.
    pub name: String,
    #[serde(default)]
    pub instructions: Option<String>,
    /// Tools per page of `tools/list`; 0 lists them all at once.
    #[serde(default)]
    pub list_page: usize,
    /// State a tool must find set first, with what a call says when it is not.
    #[serde(default)]
    pub flags: BTreeMap<String, Flag>,
    /// The server's data, by collection: each item a table of fields.
    #[serde(default)]
    pub collections: BTreeMap<String, Vec<Map<String, Value>>>,
    #[serde(rename = "tool")]
    pub tools: Vec<Tool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub missing: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    /// Items of a collection, matched, filtered and paged.
    List,
    /// How many items match.
    Count,
    /// One item by its key.
    Get,
    /// Fields of one item, set from the arguments.
    Update,
    /// A new item from the arguments.
    Create,
    /// Only `text`, filled from the arguments.
    Text,
    /// Accepts or rejects one integer argument against a data-defined predicate (`check`).
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// A `0x…` address.
    Hex,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// Its JSON Schema of the arguments, as JSON text.
    pub schema: String,
    #[serde(default)]
    pub read_only: bool,
    pub act: Act,
    #[serde(default)]
    pub collection: Option<String>,
    /// An argument naming the collection instead (a database's table).
    #[serde(default)]
    pub collection_arg: Option<String>,
    /// The field an item is found by, and the argument giving it (the same name unless `arg`).
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub arg: Option<String>,
    #[serde(default)]
    pub format: Option<Format>,
    /// With a hex key: an item's field giving its size, so an address anywhere inside it finds
    /// it, as a disassembler finds the function containing an address.
    #[serde(default)]
    pub range: Option<String>,
    /// One item as the answer shows it: `{field}` from the item, else from the arguments; `{*}`
    /// is the whole item as JSON.
    #[serde(default)]
    pub render: Option<String>,
    /// The answer of `text`, `update` and `create`, filled the same way.
    #[serde(default)]
    pub text: Option<String>,
    /// What a `get` or `update` says when no item has the key.
    #[serde(default)]
    pub missing: Option<String>,
    /// Item fields that must equal the argument of the same value, when it is given.
    #[serde(default)]
    pub matches: BTreeMap<String, String>,
    /// An argument holding an object of field = value, all of which an item must have.
    #[serde(default)]
    pub where_arg: Option<String>,
    #[serde(default)]
    pub filter: Option<Filter>,
    #[serde(default)]
    pub page: Option<Page>,
    /// For `update` and `create`: item field = the argument that sets it.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// For `update` and `create`: an object argument whose every field is set on the item.
    #[serde(default)]
    pub merge_arg: Option<String>,
    /// For `update` and `create`: fields set to these values first, so arguments can override
    /// them (a new issue's state, a close tool's `closed`).
    #[serde(default)]
    pub constants: Map<String, Value>,
    /// For `create`: the field numbering new items, `id` unless named.
    #[serde(default)]
    pub id_field: Option<String>,
    #[serde(default)]
    pub requires: Option<String>,
    #[serde(default)]
    pub sets: Option<String>,
    /// For a `check` act: the predicate a candidate argument is tested against.
    #[serde(default)]
    pub check: Option<CheckSpec>,
    /// The first call fails with this, as a busy server's would.
    #[serde(default)]
    pub fail_first: Option<String>,
    #[serde(default)]
    pub delay_ms: u64,
}

/// A case-insensitive substring search of `fields` for the argument `arg`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub arg: String,
    pub fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub size: usize,
    /// The argument naming where the page starts: an offset, or a cursor the last page gave.
    pub arg: String,
    pub kind: PageKind,
    /// An argument that may ask for fewer (or more, up to `size`) items.
    #[serde(default)]
    pub limit: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    Offset,
    Cursor,
}

/// A `check` act's predicate: the integer argument `arg` is accepted when
/// `(arg * a + b) mod m == target`, as a disassembled key check would read. A passing candidate is
/// logged to the `into` collection as `{field: value}`, so a recorded run shows it was solved.
// CEILING: one predicate shape (affine-mod over a single integer argument). Upgrade path: a small
// expression language if a task ever needs another shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub arg: String,
    pub a: i64,
    pub b: i64,
    pub m: i64,
    pub target: i64,
    pub into: String,
    pub field: String,
    /// What the tool answers on a pass and on a reject.
    pub ok: String,
    pub no: String,
}

pub fn load(path: &Path) -> Definition {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let definition: Definition =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for tool in &definition.tools {
        serde_json::from_str::<Value>(&tool.schema)
            .unwrap_or_else(|e| panic!("{}: tool {}'s schema: {e}", path.display(), tool.name));
    }
    definition
}
