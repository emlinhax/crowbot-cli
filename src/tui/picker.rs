//! `/models` inside a session: the catalog as an aligned list in a card in place of the editor.
//! ↑↓ move, → or Enter picks the model for the rest of the session, Esc or ← closes.

use crate::api::models::Catalog;
use crate::commands::models;
use crate::limits;
use crate::text::styled::{Line, Style};
use crate::text::table::{self, Align};
use crate::tui::boxed::boxed;
use crate::tui::keymap::Action;
use crate::tui::ui;

/// Cells between columns.
const GAP: usize = 2;
/// The selection marker before each row: `› ` or two spaces.
const MARKER: usize = 2;

pub struct Picker {
    ids: Vec<String>,
    /// Header first, then one row per model, as the table's cells.
    rows: Vec<Vec<Line>>,
    selected: usize,
    note: String,
}

pub enum Step {
    Stay,
    /// Use the model with this id.
    Pick(String),
    Close,
}

impl Picker {
    pub fn new(catalog: &Catalog, current: &str) -> Self {
        let mark = models::current_mark();
        let mut header = vec![Line::default()];
        header.extend(
            models::titles()
                .into_iter()
                .map(|t| Line::styled(t, Style::fg("muted").bold())),
        );
        let mut rows = vec![header];
        for m in &catalog.models {
            let mut row = vec![if m.id == current {
                Line::styled(mark, Style::fg("accent"))
            } else {
                Line::default()
            }];
            row.extend(models::cells(m).into_iter().map(Line::plain));
            rows.push(row);
        }
        Self {
            ids: catalog.models.iter().map(|m| m.id.clone()).collect(),
            rows,
            selected: catalog
                .models
                .iter()
                .position(|m| m.id == current)
                .unwrap_or(0),
            note: models::source_note(catalog),
        }
    }

    pub fn key(&mut self, action: Option<Action>) -> Step {
        let last = self.ids.len().saturating_sub(1);
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => self.selected = (self.selected + 1).min(last),
            Some(Action::Right | Action::Submit) => {
                return self
                    .ids
                    .get(self.selected)
                    .map_or(Step::Close, |id| Step::Pick(id.clone()));
            }
            Some(Action::Left | Action::Escape) => return Step::Close,
            _ => {}
        }
        Step::Stay
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let words = &ui::get().picker;
        let mut aligns = vec![Align::Left];
        aligns.extend(models::aligns());
        // Column 0 here is the current-model mark, ahead of the catalog's columns.
        let drop: Vec<usize> = models::drop_order().into_iter().map(|i| i + 1).collect();
        // The box's borders take four cells.
        let inner = width.saturating_sub(4 + MARKER);
        let mut lines = table::plain(&self.rows, &aligns, &drop, inner, GAP).into_iter();
        let mut body = Vec::new();
        if let Some(header) = lines.next() {
            let mut line = Line::plain(" ".repeat(MARKER));
            line.extend(header);
            body.push(line);
        }
        let rows = limits::get().tui.palette_rows.value.max(1);
        let start = (self.selected + 1).saturating_sub(rows);
        for (i, row) in lines.enumerate().skip(start).take(rows) {
            body.push(if i == self.selected {
                let chosen = Style::fg("accent").bold();
                let mut line = Line::styled("› ", chosen.clone());
                line.push(row.text(), chosen);
                line
            } else {
                let mut line = Line::plain(" ".repeat(MARKER));
                line.extend(row);
                line
            });
        }
        body.push(Line::default());
        body.push(Line::styled(&self.note, Style::fg("muted")));
        let title = Line::styled(&words.title, Style::default().bold());
        // As wide as the list needs, not the whole terminal.
        let needed = body.iter().map(Line::width).max().unwrap_or(0) + 4;
        let mut out = boxed(title, &body, needed.min(width));
        out.push(Line::styled(format!(" {}", words.hint), Style::fg("muted")).truncate(width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{self as api, Source};

    fn catalog() -> Catalog {
        Catalog {
            models: api::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    #[test]
    fn opens_on_the_current_model_and_picks_with_enter_or_right() {
        let catalog = catalog();
        let second = catalog.models[1].id.clone();
        let mut picker = Picker::new(&catalog, &second);
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Pick(id) if id == second));
        picker.key(Some(Action::Up));
        let first = catalog.models[0].id.clone();
        assert!(matches!(picker.key(Some(Action::Right)), Step::Pick(id) if id == first));
        assert!(matches!(picker.key(Some(Action::Escape)), Step::Close));
    }

    #[test]
    fn rows_line_up_inside_the_box_at_any_width() {
        let catalog = catalog();
        let picker = Picker::new(&catalog, "crow-2");
        for width in [50, 80, 140] {
            let lines = picker.render(width);
            assert!(lines.iter().all(|l| l.width() <= width), "width {width}");
            let text: Vec<String> = lines.iter().map(Line::text).collect();
            assert!(text[1].contains("model"), "{text:?}");
            assert!(
                text.iter().any(|l| l.starts_with("│ › ●  crow-2")),
                "{text:?}"
            );
        }
    }
}
