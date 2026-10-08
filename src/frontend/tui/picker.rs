//! `/models` inside a session: the catalog as an aligned list in a card in place of the editor.
//! ↑↓ move, → or Enter picks the model for the rest of the session, Esc or ← closes.

use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::models::Catalog;
use crate::models::table as models;
use crate::text::styled::{Line, Style};
use crate::text::table::{self, GAP};

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
        let rows = models::rows(catalog, current);
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
        match action {
            Some(step @ (Action::Up | Action::Down)) => {
                self.selected = rows::step(self.selected, step, self.ids.len(), false);
            }
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
        let inner = boxed::inner(width).saturating_sub(rows::MARK);
        let mut lines = table::plain(
            &self.rows,
            &models::aligns(),
            &models::drop_order(),
            inner,
            GAP,
        )
        .into_iter();
        let mut body = Vec::new();
        if let Some(header) = lines.next() {
            let mut line = Line::plain(" ".repeat(rows::MARK));
            line.extend(header);
            body.push(line);
        }
        body.extend(rows::marked(lines, self.selected));
        body.push(Line::default());
        body.push(Line::styled(&self.note, Style::fg("muted")));
        let title = Line::styled(&words.title, Style::default().bold());
        // As wide as the list needs, not the whole terminal.
        let needed = boxed::outer(body.iter().map(Line::width).max().unwrap_or(0));
        let mut out = boxed(title, &body, needed.min(width));
        out.push(boxed::hint(&words.hint, width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{self as catalogs, Source};

    fn catalog() -> Catalog {
        Catalog {
            models: catalogs::snapshot(),
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
