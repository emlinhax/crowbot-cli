//! The popup listing session commands while `/name` is typed: ↑↓ pick, Tab completes the name
//! so arguments can follow, Enter runs the pick, Esc hides it until the text changes.

use crate::commands::{self, Scope, Spec};
use crate::limits;
use crate::text::styled::{Line, Style, width};
use crate::tui::boxed::boxed;
use crate::tui::keymap::Action;
use crate::tui::ui;

#[derive(Default)]
pub struct Palette {
    selected: usize,
    /// The text it last answered for; any change resets the pick and a dismissal.
    seen: String,
    dismissed: bool,
}

/// What a key did while the popup was up.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Not the popup's key (or it is closed): the editor gets it.
    Ignored,
    Handled,
    /// Replace the editor's text with this.
    Complete(String),
    /// Run this command.
    Run(String),
}

/// Session commands whose name starts with what follows the `/`, alphabetically so an exact
/// name comes before longer ones sharing it (`/mode` before `/models`).
pub fn matches(text: &str) -> Vec<&'static Spec> {
    let Some(prefix) = text
        .strip_prefix(Scope::Session.prefix())
        .filter(|p| !p.contains(char::is_whitespace))
    else {
        return Vec::new();
    };
    let mut found: Vec<&'static Spec> = commands::available(Scope::Session)
        .map(|c| c.spec())
        .filter(|s| !s.hidden && s.name.starts_with(prefix))
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

impl Palette {
    /// The commands to show for `text`; empty when the popup is closed.
    pub fn open(&mut self, text: &str) -> Vec<&'static Spec> {
        if text != self.seen {
            text.clone_into(&mut self.seen);
            self.selected = 0;
            self.dismissed = false;
        }
        if self.dismissed {
            return Vec::new();
        }
        let found = matches(text);
        self.selected = self.selected.min(found.len().saturating_sub(1));
        found
    }

    pub fn key(&mut self, action: Action, text: &str) -> Step {
        let found = self.open(text);
        let Some(last) = found.len().checked_sub(1) else {
            return Step::Ignored;
        };
        match action {
            Action::Up => self.selected = self.selected.checked_sub(1).unwrap_or(last),
            Action::Down => {
                self.selected = if self.selected == last {
                    0
                } else {
                    self.selected + 1
                }
            }
            Action::Tab => {
                return Step::Complete(
                    Scope::Session.invoke(&format!("{} ", found[self.selected].name)),
                );
            }
            Action::Submit => return Step::Run(found[self.selected].name.clone()),
            Action::Escape => self.dismissed = true,
            _ => return Step::Ignored,
        }
        Step::Handled
    }

    pub fn render(&self, found: &[&Spec], width: usize) -> Vec<Line> {
        let rows = limits::get().tui.palette_rows.value.max(1);
        let start = (self.selected + 1).saturating_sub(rows);
        let name_width = found.iter().map(|s| self.width_of(s)).max().unwrap_or(0);
        let body: Vec<Line> = found
            .iter()
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(i, spec)| {
                let chosen = i == self.selected;
                let (mark, name_style) = if chosen {
                    ("› ", Style::fg("accent").bold())
                } else {
                    ("  ", Style::default())
                };
                let mut line = Line::styled(mark, name_style.clone());
                line.push(Scope::Session.invoke(&spec.name), name_style);
                let pad = name_width - self.width_of(spec) + 2;
                line.push(" ".repeat(pad), Style::default());
                line.push(&spec.summary, Style::fg("muted"));
                line
            })
            .collect();
        let title = Line::styled(&ui::get().palette.title, Style::fg("muted").bold());
        boxed(title, &body, width)
    }

    fn width_of(&self, spec: &Spec) -> usize {
        width(&Scope::Session.invoke(&spec.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(found: &[&Spec]) -> Vec<String> {
        found.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn filters_session_commands_by_prefix_until_a_space() {
        assert_eq!(names(&matches("/mo")), ["mode", "models"]);
        assert!(names(&matches("/")).contains(&"quit".to_owned()));
        // Hidden and command-line-only commands never show.
        assert!(!names(&matches("/")).contains(&"keytest".to_owned()));
        assert!(!names(&matches("/")).contains(&"signup".to_owned()));
        assert!(matches("/mode plan").is_empty());
        assert!(matches("mo").is_empty());
        assert!(matches("/zzz").is_empty());
    }

    #[test]
    fn arrows_pick_tab_completes_and_enter_runs() {
        let mut p = Palette::default();
        assert_eq!(
            p.key(Action::Tab, "/mo"),
            Step::Complete("/mode ".to_owned())
        );
        assert_eq!(p.key(Action::Down, "/mo"), Step::Handled);
        assert_eq!(p.key(Action::Submit, "/mo"), Step::Run("models".to_owned()));
        // Down past the end wraps to the top; Up from the top wraps to the end.
        p.key(Action::Down, "/mo");
        assert_eq!(p.key(Action::Submit, "/mo"), Step::Run("mode".to_owned()));
        p.key(Action::Up, "/mo");
        assert_eq!(p.key(Action::Submit, "/mo"), Step::Run("models".to_owned()));
        assert_eq!(p.key(Action::Left, "/mo"), Step::Ignored);
        assert_eq!(p.key(Action::Submit, "hello"), Step::Ignored);
    }

    #[test]
    fn escape_hides_it_until_the_text_changes() {
        let mut p = Palette::default();
        p.key(Action::Down, "/mo");
        assert_eq!(p.key(Action::Escape, "/mo"), Step::Handled);
        assert!(p.open("/mo").is_empty());
        assert_eq!(p.key(Action::Submit, "/mo"), Step::Ignored);
        // New text reopens it with the first match picked.
        assert_eq!(p.key(Action::Submit, "/m"), Step::Run("mode".to_owned()));
    }

    #[test]
    fn renders_a_box_with_the_pick_marked_and_scrolls_to_it() {
        let mut p = Palette::default();
        let found = p.open("/mo");
        p.key(Action::Down, "/mo");
        let text: Vec<String> = p.render(&found, 60).iter().map(Line::text).collect();
        assert!(text[0].starts_with("╭ commands"), "{text:?}");
        assert!(text[1].starts_with("│   /mode    "), "{text:?}");
        assert!(text[2].starts_with("│ › /models  "), "{text:?}");
        assert!(text.iter().all(|l| width(l) == 60));

        let rows = limits::get().tui.palette_rows.value;
        let found = p.open("/");
        for _ in 0..found.len() - 1 {
            p.key(Action::Down, "/");
        }
        let lines = p.render(&found, 60);
        assert_eq!(lines.len(), rows.min(found.len()) + 2);
        assert!(lines[lines.len() - 2].text().contains('›'));
    }
}
