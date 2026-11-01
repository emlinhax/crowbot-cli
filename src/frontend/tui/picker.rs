//! A table to pick one row of, in a card in place of the editor: `/models` picks the model or
//! preset for the rest of the session (`/presets` the presets alone, with their rules), `/resume`
//! a session to go on with. ↑↓ move, → or Enter picks, Esc or ← closes.

use std::path::{Path, PathBuf};

use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::models::Catalog;
use crate::models::preset::Preset;
use crate::models::preset_table as presets;
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
    title: &'static str,
    hint: &'static str,
}

enum Pick {
    /// A model or a preset, by the name either is chosen by.
    Model(String),
    /// Write a new preset to edit.
    NewPreset,
    Session(PathBuf),
    /// A row to read, not to pick: a heading, a preset's rule.
    Label,
}

/// What `/models` lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// The models, then the presets.
    All,
    /// The presets alone, each with its rules.
    Presets,
}

impl Picker {
    /// The models then the presets `listed`, or those alone with their rules and where they live
    /// (`dir`), opened on the one in use.
    pub fn models(
        catalog: &Catalog,
        listed: &[Preset],
        dir: &Path,
        current: &str,
        show: Show,
    ) -> Self {
        let words = &ui::get().picker;
        let (mut picks, mut rows, aligns, drop, note) = match show {
            Show::All => (
                (catalog.models.iter())
                    .map(|m| Pick::Model(m.id.clone()))
                    .collect(),
                models::rows(catalog, current),
                models::aligns(),
                models::drop_order(),
                models::source_note(catalog),
            ),
            Show::Presets => (
                Vec::new(),
                vec![presets::header()],
                Vec::new(),
                Vec::new(),
                presets::folder_note(dir, listed.is_empty()),
            ),
        };
        if show == Show::All {
            picks.push(Pick::Label);
            rows.push(presets::heading());
        }
        for preset in listed {
            picks.push(Pick::Model(preset.id.clone()));
            rows.push(presets::row(preset, current));
            if show == Show::Presets {
                for rule in presets::rule_rows(preset) {
                    picks.push(Pick::Label);
                    rows.push(rule);
                }
            }
        }
        picks.push(Pick::NewPreset);
        rows.push(presets::new_row());
        let selected = (picks.iter())
            .position(|p| matches!(p, Pick::Model(id) if id == current))
            .or_else(|| picks.iter().position(Pick::pickable))
            .unwrap_or(0);
        Self {
            picks,
            rows,
            aligns,
            drop,
            selected,
            note: Some(note),
            title: match show {
                Show::All => &words.title,
                Show::Presets => &words.presets_title,
            },
            hint: &words.hint,
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
            title: &words.title,
            hint: &words.hint,
        }
    }
}

impl Pick {
    fn pickable(&self) -> bool {
        !matches!(self, Self::Label)
    }
}

impl Picker {
    /// Moves the selection a row, over any that cannot be picked; at an end it stays.
    fn step(&mut self, action: Action) {
        let mut at = self.selected;
        loop {
            let next = rows::step(at, action, self.picks.len(), false);
            if next == at {
                return;
            }
            at = next;
            if self.picks[at].pickable() {
                self.selected = at;
                return;
            }
        }
    }
}

impl Card for Picker {
    fn key(&mut self, action: Option<Action>, _key: &KeyEvent) -> Next {
        match action {
            Some(step @ (Action::Up | Action::Down)) => self.step(step),
            Some(Action::Right | Action::Submit) => {
                return match self.picks.get(self.selected) {
                    Some(Pick::Model(id)) => Next::UseModel(id.clone()),
                    Some(Pick::NewPreset) => Next::NewPreset,
                    Some(Pick::Session(path)) => Next::Resume(path.clone()),
                    Some(Pick::Label) => Next::Stay,
                    None => Next::Close(None),
                };
            }
            Some(Action::Left | Action::Escape) => return Next::Close(None),
            _ => {}
        }
        Next::Stay
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
        // The presets sit under the models, often past the window: say there is more.
        let hidden = rows::below(self.picks.len(), self.selected);
        if hidden > 0 {
            let more = ui::get().text.more_lines.of(hidden);
            body.push(Line::styled(more, Style::fg("muted")));
        }
        if let Some(note) = &self.note {
            body.push(Line::default());
            body.push(Line::styled(note, Style::fg("muted")));
        }
        let title = Line::styled(self.title, Style::default().bold());
        // As wide as the list needs, not the whole terminal.
        let needed = boxed::outer(body.iter().map(Line::width).max().unwrap_or(0));
        let mut out = boxed(title, &body, needed.min(width));
        out.push(boxed::hint(self.hint, width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::models::preset;
    use crate::models::{self as catalogs, Source};

    const DIR: &str = "/home/u/.crowbot/presets";

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
        let mut picker = Picker::models(&catalog, &[], DIR.as_ref(), &second, Show::All);
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
        let picker = Picker::models(&catalog, &[], DIR.as_ref(), "crow-2", Show::All);
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

    fn listed() -> Vec<Preset> {
        let text = "description = \"flash\"\n[[rule]]\nwhen = { agent = [\"subagent\"] }\nmodel = \"a\"\n[[rule]]\nmodel = \"b\"";
        vec![preset::parsed("cheap", text)]
    }

    #[test]
    fn presets_follow_the_models_under_a_heading_the_selection_steps_over() {
        let catalog = catalog();
        let last = catalog.models.last().unwrap().id.clone();
        let mut picker = Picker::models(&catalog, &listed(), DIR.as_ref(), &last, Show::All);
        let text = |picker: &Picker| -> Vec<String> {
            picker.render(140).iter().map(Line::text).collect()
        };
        // Past the window, the rows below are counted.
        let shown = text(&picker);
        assert!(
            shown.iter().any(|l| l.contains("… 3 more lines")),
            "{shown:?}"
        );
        key(&mut picker, Action::Down);
        let shown = text(&picker);
        let at = |word: &str| shown.iter().position(|l| l.contains(word)).unwrap();
        assert!(
            at(&last) < at("presets") && at("presets") < at("cheap"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|l| l.contains("… 1 more line")),
            "{shown:?}"
        );
        // The rules are /presets' to show.
        assert!(!shown.iter().any(|l| l.contains("otherwise")), "{shown:?}");
        assert!(matches!(key(&mut picker, Action::Submit), Next::UseModel(id) if id == "cheap"));
        key(&mut picker, Action::Down);
        assert!(matches!(key(&mut picker, Action::Submit), Next::NewPreset));
        key(&mut picker, Action::Down);
        assert!(matches!(key(&mut picker, Action::Submit), Next::NewPreset));
        key(&mut picker, Action::Up);
        key(&mut picker, Action::Up);
        assert!(matches!(key(&mut picker, Action::Submit), Next::UseModel(id) if id == last));
    }

    #[test]
    fn presets_alone_show_their_rules_and_open_on_the_one_in_use() {
        let mut picker =
            Picker::models(&catalog(), &listed(), DIR.as_ref(), "cheap", Show::Presets);
        let text: Vec<String> = picker.render(120).iter().map(Line::text).collect();
        assert!(text.iter().any(|l| l.contains("› ●  cheap")), "{text:?}");
        assert!(
            text.iter().any(|l| l.contains("agent subagent")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains(DIR)), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("crow-2")), "{text:?}");
        assert!(matches!(key(&mut picker, Action::Submit), Next::UseModel(id) if id == "cheap"));
        key(&mut picker, Action::Down);
        assert!(matches!(key(&mut picker, Action::Submit), Next::NewPreset));
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
