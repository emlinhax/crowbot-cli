//! The multi-line prompt editor: text, cursor, history, and drawing with a visible cursor.

mod history;
mod render;

use unicode_segmentation::UnicodeSegmentation;

use crate::frontend::tui::keymap::Action;
use crate::io::term::{KeyCode, KeyEvent, KeyModifiers};
use history::History;

#[derive(Default)]
pub struct Editor {
    lines: Vec<String>,
    row: usize,
    /// Byte offset in `lines[row]`, always on a grapheme boundary.
    col: usize,
    /// Earlier entries to step back through; only the session's prompt keeps them.
    history: Option<History>,
    /// Shows only this many trailing characters, for a secret typed in view of others.
    mask: Option<usize>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            ..Self::default()
        }
    }

    /// An editor that remembers the last `max` entries it handed back.
    pub fn with_history(max: usize) -> Self {
        Self {
            history: Some(History::new(max)),
            ..Self::new()
        }
    }

    /// Hides all but the last `shown` characters (spaces stay, so grouping is visible).
    pub fn masked(mut self, shown: usize) -> Self {
        self.mask = Some(shown);
        self
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_owned).collect();
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    pub fn clear(&mut self) {
        self.set_text("");
        if let Some(history) = &mut self.history {
            history.reset();
        }
    }

    /// Hands back the text for sending, remembering it in history.
    pub fn take(&mut self) -> String {
        let text = self.text();
        self.take_remembering(text)
    }

    /// Hands back the text, remembering `remembered` in its place (a command with its secrets
    /// hidden).
    pub fn take_remembering(&mut self, remembered: String) -> String {
        let text = self.text();
        if let Some(history) = &mut self.history {
            history.remember(remembered);
        }
        self.clear();
        text
    }

    /// Types the key's character, if it is a plain one (no Ctrl or Alt).
    pub fn type_key(&mut self, key: &KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if let (KeyCode::Char(c), true) = (key.code, plain) {
            self.insert(&c.to_string());
        }
    }

    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut pieces = text.split('\n');
        if let Some(first) = pieces.next() {
            self.lines[self.row].insert_str(self.col, first);
            self.col += first.len();
        }
        for piece in pieces {
            let rest = self.lines[self.row].split_off(self.col);
            self.row += 1;
            self.lines.insert(self.row, format!("{piece}{rest}"));
            self.col = piece.len();
        }
    }

    /// Applies an editing action; `false` when it does not concern the editor.
    pub fn apply(&mut self, action: Action) -> bool {
        match action {
            Action::Newline => self.insert("\n"),
            Action::Left => self.left(),
            Action::Right => self.right(),
            Action::WordLeft => {
                if self.col == 0 {
                    self.left();
                }
                while self.prev_grapheme().is_some_and(|g| !is_word(g)) {
                    self.left();
                }
                while self.prev_grapheme().is_some_and(is_word) {
                    self.left();
                }
            }
            Action::WordRight => {
                if self.next_grapheme().is_none() {
                    self.right();
                }
                while self.next_grapheme().is_some_and(|g| !is_word(g)) {
                    self.right();
                }
                while self.next_grapheme().is_some_and(is_word) {
                    self.right();
                }
            }
            Action::LineStart => self.col = 0,
            Action::LineEnd => self.col = self.lines[self.row].len(),
            Action::Backspace => {
                if self.col > 0 || self.row > 0 {
                    let end = (self.row, self.col);
                    self.left();
                    self.cut(end);
                }
            }
            Action::Delete => {
                let start = (self.row, self.col);
                self.right();
                let end = (self.row, self.col);
                (self.row, self.col) = start;
                self.cut(end);
            }
            Action::DeleteWord => {
                let end = (self.row, self.col);
                while self.col > 0 && self.prev_grapheme().is_some_and(|g| !is_word(g)) {
                    self.left();
                }
                while self.col > 0 && self.prev_grapheme().is_some_and(is_word) {
                    self.left();
                }
                if (self.row, self.col) == end {
                    self.left();
                }
                self.cut(end);
            }
            Action::KillToStart => {
                self.lines[self.row].replace_range(..self.col, "");
                self.col = 0;
            }
            Action::KillToEnd => self.lines[self.row].truncate(self.col),
            Action::Up => self.up(),
            Action::Down => self.down(),
            _ => return false,
        }
        true
    }

    fn left(&mut self) {
        if let Some(g) = self.prev_grapheme() {
            self.col -= g.len();
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn right(&mut self) {
        if let Some(g) = self.next_grapheme() {
            self.col += g.len();
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// CEILING: moves between logical lines; moving by wrapped (visual) line needs the render
    /// width here.
    fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
            self.snap();
        } else {
            let current = self.text();
            if let Some(text) = self.history.as_mut().and_then(|h| h.back(current)) {
                self.set_text(&text);
            }
        }
    }

    fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
            self.snap();
        } else if let Some(text) = self.history.as_mut().and_then(History::forward) {
            self.set_text(&text);
        }
    }

    /// Removes the text between the cursor and `end` (which lies at or after it).
    fn cut(&mut self, end: (usize, usize)) {
        let (row, col) = (self.row, self.col);
        if end.0 == row {
            self.lines[row].replace_range(col..end.1, "");
            return;
        }
        let tail = self.lines[end.0][end.1..].to_owned();
        self.lines[row].truncate(col);
        self.lines[row].push_str(&tail);
        self.lines.drain(row + 1..=end.0);
    }

    fn prev_grapheme(&self) -> Option<&str> {
        self.lines[self.row][..self.col].graphemes(true).next_back()
    }

    fn next_grapheme(&self) -> Option<&str> {
        self.lines[self.row][self.col..].graphemes(true).next()
    }

    /// Moves the cursor back to the grapheme boundary it may have landed inside.
    fn snap(&mut self) {
        let line = &self.lines[self.row];
        while !line.is_char_boundary(self.col) {
            self.col -= 1;
        }
        let boundary = line
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|i| *i <= self.col)
            .last()
            .unwrap_or(0);
        if self.col != line.len() {
            self.col = boundary;
        }
    }
}

fn is_word(g: &str) -> bool {
    g.chars().any(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::styled::Line;

    fn typed(text: &str) -> Editor {
        let mut e = Editor::with_history(10);
        e.insert(text);
        e
    }

    #[test]
    fn a_masked_editor_shows_only_the_tail_and_keeps_the_cursor() {
        let mut e = Editor::new().masked(4);
        e.insert("1234 5678 9012 3456");
        assert_eq!(e.text(), "1234 5678 9012 3456");
        let prompt = Line::plain("> ");
        let shown = e.render(40, 3, &prompt, "")[0].text();
        assert_eq!(shown, "> •••• •••• •••• 3456 ");
        e.apply(Action::LineStart);
        let tagged = e.render(40, 3, &prompt, "")[0].to_tagged();
        assert!(tagged.starts_with("> [reverse]•[/]•••"), "{tagged}");
    }

    #[test]
    fn inserts_moves_and_deletes_by_grapheme() {
        let mut e = typed("héllo");
        e.apply(Action::Left);
        e.apply(Action::Left);
        e.insert("X");
        assert_eq!(e.text(), "hélXlo");
        e.apply(Action::LineStart);
        e.apply(Action::Right);
        e.apply(Action::Right);
        e.apply(Action::Backspace);
        assert_eq!(e.text(), "hlXlo");
        e.apply(Action::Delete);
        assert_eq!(e.text(), "hXlo");
    }

    #[test]
    fn newlines_split_and_backspace_joins() {
        let mut e = typed("abcd");
        e.apply(Action::Left);
        e.apply(Action::Left);
        e.apply(Action::Newline);
        assert_eq!(e.text(), "ab\ncd");
        e.apply(Action::Backspace);
        assert_eq!(e.text(), "abcd");
        e.insert("1\n2\n3");
        assert_eq!(e.text(), "ab1\n2\n3cd");
    }

    #[test]
    fn words_and_kills() {
        let mut e = typed("cargo test --all");
        e.apply(Action::DeleteWord);
        assert_eq!(e.text(), "cargo test --");
        e.apply(Action::WordLeft);
        e.apply(Action::KillToEnd);
        assert_eq!(e.text(), "cargo ");
        e.apply(Action::KillToStart);
        assert_eq!(e.text(), "");
    }

    #[test]
    fn history_keeps_the_draft() {
        let mut e = Editor::with_history(10);
        e.insert("first");
        e.take();
        e.insert("second");
        e.take();
        e.insert("draft");
        e.apply(Action::Up);
        assert_eq!(e.text(), "second");
        e.apply(Action::Up);
        assert_eq!(e.text(), "first");
        e.apply(Action::Down);
        e.apply(Action::Down);
        assert_eq!(e.text(), "draft");
    }

    #[test]
    fn renders_wrapped_with_the_cursor_and_scrolls_to_it() {
        let e = typed("abcdefgh");
        let prompt = Line::plain("› ");
        let lines: Vec<String> = e
            .render(7, 5, &prompt, "")
            .iter()
            .map(Line::to_tagged)
            .collect();
        assert_eq!(lines, vec!["› abcd", "  efgh", "  [reverse] [/]"]);
        let tall = typed("1\n2\n3\n4\n5\n6");
        let shown: Vec<String> = tall
            .render(20, 3, &prompt, "")
            .iter()
            .map(Line::text)
            .collect();
        assert_eq!(shown, vec!["  4", "  5", "  6 "]);
    }

    #[test]
    fn an_empty_editor_shows_the_placeholder() {
        let e = Editor::with_history(10);
        let lines = e.render(40, 5, &Line::plain("› "), "Ask crowbot");
        assert_eq!(lines[0].to_tagged(), "› [reverse] [/][muted]Ask crowbot[/]");
    }
}
