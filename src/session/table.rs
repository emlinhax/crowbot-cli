//! This project's sessions as a table, as the session's `/resume` picker shows them
//! (data/session_table.toml).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::io::clock;
use crate::session::list::Summary;
use crate::text::styled::{Line, Style};
use crate::text::table::Align;

static DATA: LazyLock<Data> = data::load!("session_table.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    drop: Vec<String>,
    column: Vec<Column>,
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
struct Text {
    time: String,
}

/// A header, then one row per session; column 0 is left for a picker's mark.
pub fn rows(sessions: &[Summary]) -> Vec<Vec<Line>> {
    let mut header = vec![Line::default()];
    header.extend(
        DATA.column
            .iter()
            .map(|c| Line::styled(&c.title, Style::fg("muted").bold())),
    );
    let mut rows = vec![header];
    for session in sessions {
        let mut row = vec![Line::default()];
        row.extend(
            DATA.column
                .iter()
                .map(|c| Line::plain(cell(session, &c.id))),
        );
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

fn cell(session: &Summary, column: &str) -> String {
    match column {
        "when" => session
            .modified
            .map(|at| clock::local(at, &DATA.text.time))
            .unwrap_or_default(),
        "prompts" => session.prompts.to_string(),
        // One line of it: a prompt can run to many.
        "prompt" => session
            .first_prompt
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
        other => {
            debug_assert!(false, "unknown column `{other}` in data/session_table.toml");
            String::new()
        }
    }
}
