//! Highlighting with the left button, as a terminal's own selection does: a drag marks the cells
//! from where it started to where the pointer is, row by row, and letting go copies them. It
//! stands in for the terminal's, which the mouse reports take away.
//! CEILING: it marks screen cells, so a wrapped line copies as the rows it shows and a drag past
//! the top or bottom does not scroll.

use unicode_segmentation::UnicodeSegmentation;

use crate::text::styled::{self, Line};

/// A screen cell, zero-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug)]
pub struct Selection {
    from: Cell,
    to: Cell,
}

impl Selection {
    pub fn start(at: Cell) -> Self {
        Self { from: at, to: at }
    }

    pub fn extend(&mut self, to: Cell) {
        self.to = to;
    }

    /// Whether it covers more than the cell it started on; a press let go where it began is a
    /// click.
    pub fn dragged(&self) -> bool {
        self.from != self.to
    }

    /// The columns of `row` it covers, both ends included.
    fn columns(&self, row: usize) -> Option<(usize, usize)> {
        let (first, last) = if self.from <= self.to {
            (self.from, self.to)
        } else {
            (self.to, self.from)
        };
        if row < first.row || row > last.row {
            return None;
        }
        let start = if row == first.row { first.column } else { 0 };
        let end = if row == last.row {
            last.column
        } else {
            usize::MAX
        };
        Some((start, end))
    }

    /// Shows the covered cells reversed.
    pub fn paint(&self, rows: &mut [Line]) {
        for (i, row) in rows.iter_mut().enumerate() {
            if let Some(range) = self.columns(i) {
                *row = painted(row, range);
            }
        }
    }

    /// The covered text, one line per row, each without the spaces that end it.
    pub fn text(&self, rows: &[Line]) -> String {
        let mut lines = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if let Some(range) = self.columns(i) {
                let line: String = cells(row)
                    .into_iter()
                    .filter(|c| c.within(range))
                    .map(|c| c.text)
                    .collect();
                lines.push(line.trim_end().to_owned());
            }
        }
        lines.join("\n")
    }
}

/// One grapheme of a row: its text, its span and the columns it takes.
struct Grapheme<'a> {
    text: &'a str,
    span: usize,
    left: usize,
    right: usize,
}

impl Grapheme<'_> {
    /// Whether any of its columns is in `start..=end`, so a wide character is taken whole.
    fn within(&self, (start, end): (usize, usize)) -> bool {
        self.left <= end && self.right >= start
    }
}

fn cells(row: &Line) -> Vec<Grapheme<'_>> {
    let mut at = 0;
    let mut out = Vec::new();
    for (span, s) in row.spans.iter().enumerate() {
        for text in s.text.graphemes(true) {
            let width = styled::width(text).max(1);
            out.push(Grapheme {
                text,
                span,
                left: at,
                right: at + width - 1,
            });
            at += width;
        }
    }
    out
}

fn painted(row: &Line, range: (usize, usize)) -> Line {
    let mut out = Line::default();
    for cell in cells(row) {
        let mut style = row.spans[cell.span].style.clone();
        if cell.within(range) {
            style.reverse = !style.reverse;
        }
        out.push(cell.text, style);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::styled::Style;

    fn at(row: usize, column: usize) -> Cell {
        Cell { row, column }
    }

    fn rows() -> Vec<Line> {
        vec![
            Line::plain("first row   "),
            Line::plain("second"),
            Line::plain("third row"),
        ]
    }

    #[test]
    fn a_drag_copies_from_where_it_started_to_where_it_ends_either_way() {
        let mut selection = Selection::start(at(0, 6));
        selection.extend(at(2, 4));
        assert_eq!(selection.text(&rows()), "row\nsecond\nthird");
        let mut backwards = Selection::start(at(2, 4));
        backwards.extend(at(0, 6));
        assert_eq!(backwards.text(&rows()), "row\nsecond\nthird");
        let mut within = Selection::start(at(1, 4));
        within.extend(at(1, 1));
        assert_eq!(within.text(&rows()), "econ");
    }

    #[test]
    fn letting_go_where_it_started_is_a_click() {
        let mut selection = Selection::start(at(1, 2));
        assert!(!selection.dragged());
        selection.extend(at(1, 3));
        assert!(selection.dragged());
        selection.extend(at(1, 2));
        assert!(!selection.dragged());
    }

    #[test]
    fn the_covered_cells_show_reversed_and_keep_their_text_and_colours() {
        let mut row = Line::styled("ab", Style::fg("accent"));
        row.push("cd", Style::default());
        let mut rows = vec![row, Line::plain("untouched")];
        let mut selection = Selection::start(at(0, 1));
        selection.extend(at(0, 2));
        selection.paint(&mut rows);
        assert_eq!(
            rows[0].to_tagged(),
            "[accent]a[/][accent+reverse]b[/][reverse]c[/]d"
        );
        assert_eq!(rows[1].to_tagged(), "untouched");
    }

    #[test]
    fn a_wide_character_is_taken_whole_from_either_of_its_cells() {
        let rows = vec![Line::plain("a漢b")];
        let mut selection = Selection::start(at(0, 2));
        selection.extend(at(0, 3));
        assert_eq!(selection.text(&rows), "漢b");
    }
}
