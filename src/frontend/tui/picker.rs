//! A table to pick one row of, in a card in place of the editor: `/models` picks a model (the
//! roles card in roles.rs drives it for a subagent), `/resume` a session to go on with. ↑↓ move,
//! → or Enter picks, Esc or ← closes.

use std::path::PathBuf;

use crate::agent::roles::RoleId;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::models::Catalog;
use crate::models::table as models;
use crate::session::list::Summary;
use crate::session::table as sessions;
use crate::text::styled::{Line, Style};
use crate::text::table::{self, Align, GAP};

pub struct Picker {
    /// What each row picks, in row order.
    picks: Vec<Pick>,
    /// Header first, then one row per pick, as the table's cells.
    rows: Vec<Vec<Line>>,
    aligns: Vec<Align>,
    drop: Vec<usize>,
    selected: usize,
    note: Option<String>,
    title: String,
    hint: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    Model(String),
    Session(PathBuf),
    /// Drop a subagent's own choice and take the one it inherits.
    Inherit,
    Role(RoleId),
    /// An effort level; `None` inherits.
    Effort(Option<String>),
}

/// What a key did to the list.
pub enum Chose {
    Stay,
    Back,
    Picked(Pick),
}

/// A table's layout, header row first, for `Picker::table`.
pub struct Table {
    pub rows: Vec<Vec<Line>>,
    pub aligns: Vec<Align>,
    pub drop: Vec<usize>,
}

impl Picker {
    /// `picks`, one per row of `table` after its header, opened on `selected`.
    pub fn table(
        picks: Vec<Pick>,
        table: Table,
        selected: usize,
        (title, hint): (String, String),
        note: Option<String>,
    ) -> Self {
        Self {
            picks,
            rows: table.rows,
            aligns: table.aligns,
            drop: table.drop,
            selected,
            note,
            title,
            hint,
        }
    }

    /// The model catalog, opened on `current`; with `inherit`, a first row that inherits, its
    /// cells those words.
    pub fn models_for(catalog: &Catalog, current: Option<&str>, inherit: Option<String>) -> Self {
        let mut picker = Self::models(catalog, current.unwrap_or_default());
        if let Some(words) = inherit {
            let columns = picker.rows.first().map_or(1, Vec::len);
            let mut row = vec![Line::default(); columns];
            row[columns.min(2) - 1] = Line::styled(&words, Style::fg("muted"));
            picker.rows.insert(1, row);
            picker.picks.insert(0, Pick::Inherit);
            picker.selected = match current {
                Some(_) => picker.selected + 1,
                None => 0,
            };
        }
        picker
    }

    /// The same list under another title and hint.
    pub fn titled(self, title: String, hint: String) -> Self {
        Self {
            title,
            hint,
            ..self
        }
    }

    /// Moves or picks for `action`.
    pub fn choose(&mut self, action: Option<Action>) -> Chose {
        match action {
            Some(step @ (Action::Up | Action::Down)) => {
                self.selected = rows::step(self.selected, step, self.picks.len(), false);
                Chose::Stay
            }
            Some(Action::Right | Action::Submit) => match self.picks.get(self.selected) {
                Some(pick) => Chose::Picked(pick.clone()),
                None => Chose::Back,
            },
            Some(Action::Left | Action::Escape) => Chose::Back,
            _ => Chose::Stay,
        }
    }

    /// The model catalog, opened on the model in use.
    pub fn models(catalog: &Catalog, current: &str) -> Self {
        let words = &ui::get().picker;
        Self {
            picks: catalog
                .models
                .iter()
                .map(|m| Pick::Model(m.id.clone()))
                .collect(),
            rows: models::rows(catalog, current),
            aligns: models::aligns(),
            drop: models::drop_order(),
            selected: catalog
                .models
                .iter()
                .position(|m| m.id == current)
                .unwrap_or(0),
            note: Some(models::source_note(catalog)),
            title: words.title.clone(),
            hint: words.hint.clone(),
        }
    }

    /// Sessions to go on with, newest first.
    pub fn sessions(list: Vec<Summary>) -> Self {
        let words = &ui::get().resume;
        Self {
            rows: sessions::rows(&list),
            picks: list.into_iter().map(|s| Pick::Session(s.path)).collect(),
            aligns: sessions::aligns(),
            drop: sessions::drop_order(),
            selected: 0,
            note: None,
            title: words.title.clone(),
            hint: words.hint.clone(),
        }
    }
}

impl Card for Picker {
    fn key(&mut self, action: Option<Action>, _key: &KeyEvent) -> Next {
        match self.choose(action) {
            Chose::Stay => Next::Stay,
            Chose::Picked(Pick::Model(id)) => Next::UseModel(id),
            Chose::Picked(Pick::Session(path)) => Next::Resume(path),
            // The rest are the roles card's to act on.
            Chose::Picked(_) | Chose::Back => Next::Close(None),
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let inner = boxed::inner(width).saturating_sub(rows::MARK);
        let mut lines = table::plain(&self.rows, &self.aligns, &self.drop, inner, GAP).into_iter();
        let mut body = Vec::new();
        if let Some(header) = lines.next() {
            let mut line = Line::plain(" ".repeat(rows::MARK));
            line.extend(header);
            body.push(line);
        }
        body.extend(rows::marked(lines, self.selected));
        if let Some(note) = &self.note {
            body.push(Line::default());
            body.push(Line::styled(note, Style::fg("muted")));
        }
        let title = Line::styled(&self.title, Style::default().bold());
        // As wide as the list needs, not the whole terminal.
        let needed = boxed::outer(body.iter().map(Line::width).max().unwrap_or(0));
        let mut out = boxed(title, &body, needed.min(width));
        out.push(boxed::hint(&self.hint, width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::models::{self as catalogs, Source};

    fn key(picker: &mut Picker, action: Action) -> Next {
        picker.key(Some(action), &press(KeyCode::Null))
    }

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
        let mut picker = Picker::models(&catalog, &second);
        assert!(matches!(key(&mut picker, Action::Submit), Next::UseModel(id) if id == second));
        key(&mut picker, Action::Up);
        let first = catalog.models[0].id.clone();
        assert!(matches!(key(&mut picker, Action::Right), Next::UseModel(id) if id == first));
        assert!(matches!(
            key(&mut picker, Action::Escape),
            Next::Close(None)
        ));
    }

    #[test]
    fn rows_line_up_inside_the_box_at_any_width() {
        let catalog = catalog();
        let picker = Picker::models(&catalog, "crow-2");
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

    #[test]
    fn sessions_list_newest_first_and_pick_a_file() {
        let at = |s: &str| Some(s.parse().unwrap());
        let list = vec![
            Summary {
                path: "/s/new.jsonl".into(),
                modified: at("2026-10-16T09:00:00Z"),
                first_prompt: "fix the parser\nand its tests".into(),
                prompts: 3,
            },
            Summary {
                path: "/s/old.jsonl".into(),
                modified: at("2026-10-15T09:00:00Z"),
                first_prompt: "explain".into(),
                prompts: 1,
            },
        ];
        let mut picker = Picker::sessions(list);
        for width in [50, 80, 140] {
            let lines = picker.render(width);
            assert!(lines.iter().all(|l| l.width() <= width), "width {width}");
            let text: Vec<String> = lines.iter().map(Line::text).collect();
            assert!(
                text.iter().any(|l| l.contains("fix the parser")),
                "{text:?}"
            );
            assert!(!text.iter().any(|l| l.contains("its tests")), "{text:?}");
        }
        key(&mut picker, Action::Down);
        assert!(
            matches!(key(&mut picker, Action::Submit), Next::Resume(path) if path.ends_with("old.jsonl"))
        );
    }
}
