//! The model list as a table, as `crowbot models` prints it and the session's picker shows it
//! (data/model_table.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::models::Model;
use crate::data;
use crate::models::{Catalog, Source};
use crate::text::styled::{Line, Style};
use crate::text::table::Align;
use crate::text::template::fill;
use crate::text::units;

static DATA: LazyLock<Data> = data::load!("model_table.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    drop: Vec<String>,
    column: Vec<Column>,
    capability: Vec<Capability>,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Column {
    id: String,
    title: String,
    #[serde(default)]
    align: Align,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    id: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    current: String,
    live: String,
    cache: String,
    cache_time: String,
    snapshot: String,
    unreachable: String,
}

/// A header, then one row per model; column 0 marks the model in use.
pub fn rows(catalog: &Catalog, current: &str) -> Vec<Vec<Line>> {
    let mut header = vec![Line::default()];
    header.extend(
        DATA.column
            .iter()
            .map(|c| Line::styled(&c.title, Style::fg("muted").bold())),
    );
    let mut rows = vec![header];
    for m in &catalog.models {
        let mut row = vec![if m.id == current {
            Line::styled(&DATA.text.current, Style::fg("accent"))
        } else {
            Line::default()
        }];
        row.extend(cells(m).into_iter().map(Line::plain));
        rows.push(row);
    }
    rows
}

/// Each column's alignment, the mark column first.
pub fn aligns() -> Vec<Align> {
    let mut aligns = vec![Align::Left];
    aligns.extend(DATA.column.iter().map(|c| c.align));
    aligns
}

/// Columns to leave out when narrow, first to go first, counting the mark column.
pub fn drop_order() -> Vec<usize> {
    DATA.drop
        .iter()
        .filter_map(|id| DATA.column.iter().position(|c| c.id == *id))
        .map(|i| i + 1)
        .collect()
}

/// One model's cells, in column order.
fn cells(m: &Model) -> Vec<String> {
    DATA.column.iter().map(|c| cell(m, &c.id)).collect()
}

fn cell(m: &Model, column: &str) -> String {
    match column {
        "id" => m.id.clone(),
        "name" => m.display_name.clone(),
        "context" => units::tokens(m.context_window),
        "input" => units::usd(m.pricing.input_per_1m_usd),
        "cached" => units::usd(m.pricing.cached_input_per_1m_usd),
        "output" => units::usd(m.pricing.output_per_1m_usd),
        "max_output" => units::tokens(m.max_output_tokens),
        "capabilities" => DATA
            .capability
            .iter()
            .filter(|c| has(m, &c.id))
            .map(|c| c.label.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        other => {
            debug_assert!(false, "unknown column `{other}` in data/model_table.toml");
            String::new()
        }
    }
}

fn has(m: &Model, capability: &str) -> bool {
    match capability {
        "reasoning" => m.capabilities.reasoning,
        "tool_calling" => m.capabilities.tool_calling,
        "caching" => m.capabilities.caching,
        other => {
            debug_assert!(
                false,
                "unknown capability `{other}` in data/model_table.toml"
            );
            false
        }
    }
}

/// The mark on the model in use.
pub fn mark() -> &'static str {
    &DATA.text.current
}

/// Where the list came from, and why not from crowbot when it could not be reached.
pub fn source_note(catalog: &Catalog) -> String {
    let text = &DATA.text;
    let when = catalog
        .fetched_at
        .map(|t| t.strftime(&text.cache_time).to_string())
        .unwrap_or_default();
    let mut note = match catalog.source {
        Source::Live => text.live.clone(),
        Source::Cache => fill(&text.cache, &[("when", &when)]),
        Source::Snapshot => text.snapshot.clone(),
    };
    if let Some(why) = &catalog.note {
        note.push(' ');
        note.push_str(&fill(&text.unreachable, &[("note", why)]));
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_column_has_a_cell_and_drop_names_only_columns() {
        for m in &crate::models::snapshot() {
            assert_eq!(cells(m).len(), DATA.column.len());
            for c in &DATA.capability {
                has(m, &c.id);
            }
        }
        assert_eq!(
            drop_order().len(),
            DATA.drop.len(),
            "drop names unknown columns"
        );
    }
}
