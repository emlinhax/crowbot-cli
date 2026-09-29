//! The multi-line prompt editor: text, cursor, history, and drawing with a visible cursor.

use unicode_segmentation::UnicodeSegmentation;

use crate::text::styled::{Line, Style, width};
use crate::tui::keymap::Action;

#[derive(Default)]
pub struct Editor {
    lines: Vec<String>,
    row: usize,
    /// Byte offset in `lines[row]`, always on a grapheme boundary.
    col: usize,
    history: Vec<String>,
    /// Which history entry is shown while browsing; `None` when editing a fresh draft.
    browsing: Option<usize>,
    /// The draft set aside while browsing history.
    draft: String,
    history_max: usize,
    /// Shows only this many trailing characters, for a secret typed in view of others.
    mask: Option<usize>,
}

impl Editor {
    pub fn new(history_max: usize) -> Self {
        Self {
            lines: vec![String::new()],
            history_max,
            ..Self::default()
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
        self.browsing = None;
    }

    /// Hands back the text for sending, remembering it in history.
    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > self.history_max {
                self.history.remove(0);
            }
        }
        self.clear();
        text
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

    /// Lines to draw, wrapped to `width` with a cursor cell, scrolled to show the cursor within
    /// `max_rows`. `prompt` starts the first line; later lines are indented to match.
    pub fn render(
        &self,
        width: usize,
        max_rows: usize,
        prompt: &Line,
        placeholder: &str,
    ) -> Vec<Line> {
        let indent = Line::plain(" ".repeat(prompt.width()));
        // One column stays free so the cursor fits after the last character.
        let avail = width.saturating_sub(prompt.width() + 1).max(1);
        let mut rows: Vec<Line> = Vec::new();
        let mut cursor_row = 0;
        for (i, text) in self.lines.iter().enumerate() {
            let cursor = (i == self.row).then_some(self.col);
            let (text, cursor) = match self.mask {
                Some(shown) => mask(text, cursor, shown),
                None => (text.clone(), cursor),
            };
            let (wrapped, at) = wrap_with_cursor(&text, cursor, avail);
            if let Some(at) = at {
                cursor_row = rows.len() + at;
            }
            rows.extend(wrapped);
        }
        if self.is_empty() && !placeholder.is_empty() {
            let mut line = Line::styled(" ", Style::default().reverse());
            line.push(placeholder, Style::fg("muted"));
            rows = vec![line.truncate(avail + 1)];
        }
        let max_rows = max_rows.max(1);
        let top = (cursor_row + 1).saturating_sub(max_rows);
        rows.into_iter()
            .enumerate()
            .skip(top)
            .take(max_rows)
            .map(|(i, row)| {
                let mut line = if i == 0 {
                    prompt.clone()
                } else {
                    indent.clone()
                };
                line.extend(row);
                line
            })
            .collect()
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
            self.history_back();
        }
    }

    fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
            self.snap();
        } else {
            self.history_forward();
        }
    }

    fn history_back(&mut self) {
        let next = match self.browsing {
            None if self.history.is_empty() => return,
            None => {
                self.draft = self.text();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.browsing = Some(next);
        let text = self.history[next].clone();
        self.set_text(&text);
    }

    fn history_forward(&mut self) {
        let Some(i) = self.browsing else {
            return;
        };
        if i + 1 < self.history.len() {
            self.browsing = Some(i + 1);
            let text = self.history[i + 1].clone();
            self.set_text(&text);
        } else {
            self.browsing = None;
            let draft = std::mem::take(&mut self.draft);
            self.set_text(&draft);
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

/// Wraps one logical line by cells, drawing the cursor (reverse video) at byte `cursor`.
/// Returns the rows and which of them holds the cursor.
/// `text` with every non-space character but the last `shown` as `•`, and the cursor's byte
/// offset moved to match.
fn mask(text: &str, cursor: Option<usize>, shown: usize) -> (String, Option<usize>) {
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    let mut out = String::with_capacity(text.len());
    let mut at = None;
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        if cursor == Some(i) {
            at = Some(out.len());
        }
        if c.is_whitespace() {
            out.push(c);
            continue;
        }
        seen += 1;
        out.push(if seen + shown > total { c } else { '•' });
    }
    if cursor == Some(text.len()) {
        at = Some(out.len());
    }
    (out, at)
}

fn wrap_with_cursor(text: &str, cursor: Option<usize>, avail: usize) -> (Vec<Line>, Option<usize>) {
    let mut rows = vec![Line::default()];
    let mut used = 0;
    let mut at = None;
    let cursor_style = Style::default().reverse();
    let mut pieces: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    pieces.push((text.len(), ""));
    for (offset, g) in pieces {
        let is_cursor = cursor == Some(offset);
        let shown = if g.is_empty() { " " } else { g };
        if g.is_empty() && !is_cursor {
            break;
        }
        let w = width(shown).max(1);
        if used + w > avail && used > 0 {
            rows.push(Line::default());
            used = 0;
        }
        if is_cursor {
            at = Some(rows.len() - 1);
            rows.last_mut()
                .unwrap_or(&mut Line::default())
                .push(shown, cursor_style.clone());
        } else {
            rows.last_mut()
                .unwrap_or(&mut Line::default())
                .push(shown, Style::default());
        }
        used += w;
    }
    (rows, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Editor {
        let mut e = Editor::new(10);
        e.insert(text);
        e
    }

    #[test]
    fn a_masked_editor_shows_only_the_tail_and_keeps_the_cursor() {
        let mut e = Editor::new(0).masked(4);
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
        let mut e = Editor::new(10);
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
        let e = Editor::new(10);
        let lines = e.render(40, 5, &Line::plain("› "), "Ask crowbot");
        assert_eq!(lines[0].to_tagged(), "› [reverse] [/][muted]Ask crowbot[/]");
    }
}
