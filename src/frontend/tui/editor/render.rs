//! Drawing the editor: wrapped to the width, the cursor shown, a secret masked.

use unicode_segmentation::UnicodeSegmentation;

use super::Editor;
use crate::text::styled::{Line, Style, width};

impl Editor {
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
}

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

/// Wraps one logical line by cells, drawing the cursor (reverse video) at byte `cursor`.
/// Returns the rows and which of them holds the cursor.
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
