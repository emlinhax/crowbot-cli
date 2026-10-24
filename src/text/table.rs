//! Columns that fit a width: each starts at its widest cell, then the widest gives up one cell at a
//! time (cut with `…`) until the table fits.

use crate::config::limits;
use crate::text::styled::{Line, Style};
use crate::text::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Right,
}

/// Column widths for `rows` in `width` cells, `overhead` of which go to borders and gaps.
/// CEILING: cells are cut, never wrapped; wrapping would need rows of several lines.
fn widths(rows: &[Vec<Line>], width: usize, overhead: usize) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.width());
        }
    }
    let budget = width.saturating_sub(overhead);
    while widths.iter().sum::<usize>() > budget {
        let Some((i, &w)) = widths.iter().enumerate().max_by_key(|&(_, w)| *w) else {
            break;
        };
        if w <= limits::get().text.min_column.value {
            break;
        }
        widths[i] -= 1;
    }
    widths
}

/// The columns to show in `width`: all of them while they fit uncut, else as few left out as
/// needed, in `drop` order. What is kept may still be cut to fit.
fn keep(rows: &[Vec<Line>], drop: &[usize], width: usize, gap: usize) -> Vec<usize> {
    let natural = widths(rows, usize::MAX, 0);
    let fits = |kept: &[usize]| {
        kept.iter().map(|&i| natural[i]).sum::<usize>() + gap * kept.len().saturating_sub(1)
            <= width
    };
    let mut kept: Vec<usize> = (0..natural.len()).collect();
    for column in drop {
        if fits(&kept) {
            break;
        }
        kept.retain(|i| i != column);
    }
    kept
}

/// `cell` cut or padded to exactly `width` cells.
fn fit(cell: &Line, width: usize, align: Align) -> Line {
    let cell = cell.truncate(width);
    match align {
        Align::Left => cell.padded(width, &Style::default()),
        Align::Right => {
            let pad = " ".repeat(width.saturating_sub(cell.width()));
            let mut out = Line::plain(pad);
            out.extend(cell);
            out
        }
    }
}

/// Cells side by side: `edges` are what goes before the first, between two, and after the last,
/// all in `style`.
fn join(cells: impl IntoIterator<Item = Line>, edges: [&str; 3], style: &Style) -> Line {
    let [left, mid, right] = edges;
    let mut line = Line::styled(left, style.clone());
    for (i, cell) in cells.into_iter().enumerate() {
        if i > 0 {
            line.push(mid, style.clone());
        }
        line.extend(cell);
    }
    line.push(right, style.clone());
    line
}

/// A bordered grid, header row first, within `width` cells. Columns are cut to fit; when even
/// cut ones do not, the rightmost are left out (markdown has no better order to drop them in).
pub fn grid(rows: &[Vec<Line>], aligns: &[Align], width: usize) -> Vec<Line> {
    let natural = widths(rows, usize::MAX, 0);
    // Borders: one before each column, one after the last, and a space either side of cells.
    let overhead = |columns: usize| 3 * columns + 1;
    let narrowest = |columns: usize| -> usize {
        natural[..columns]
            .iter()
            .map(|&w| w.min(limits::get().text.min_column.value))
            .sum::<usize>()
            + overhead(columns)
    };
    let mut columns = natural.len();
    while columns > 1 && narrowest(columns) > width {
        columns -= 1;
    }
    if columns == 0 {
        return Vec::new();
    }
    let rows: Vec<Vec<Line>> = rows
        .iter()
        .map(|row| {
            (0..columns)
                .map(|i| row.get(i).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    let widths = widths(&rows, width, overhead(columns));
    let border = Style::fg("muted");
    let g = &theme::get().glyphs;
    let rule = |edges: &[String; 3]| {
        let dashes = widths
            .iter()
            .map(|w| Line::styled(g.rule.repeat(w + 2), border.clone()));
        join(dashes, edges.each_ref().map(String::as_str), &border)
    };
    let mut lines = vec![rule(&g.table_top)];
    for (r, row) in rows.iter().enumerate() {
        let cells = row.iter().zip(&widths).enumerate().map(|(i, (cell, w))| {
            let mut padded = Line::plain(" ");
            padded.extend(fit(cell, *w, aligns.get(i).copied().unwrap_or_default()));
            padded.push(" ", Style::default());
            padded
        });
        let side = g.table_side.as_str();
        lines.push(join(cells, [side, side, side], &border));
        if r == 0 && rows.len() > 1 {
            lines.push(rule(&g.table_head));
        }
    }
    lines.push(rule(&g.table_bottom));
    lines.into_iter().map(|l| l.truncate(width)).collect()
}

/// Cells between the columns of a borderless table.
pub const GAP: usize = 2;

/// Borderless rows, cells `gap` apart, fitted to `width` by leaving out columns in `drop`
/// order, then cutting.
pub fn plain(
    rows: &[Vec<Line>],
    aligns: &[Align],
    drop: &[usize],
    width: usize,
    gap: usize,
) -> Vec<Line> {
    let kept = keep(rows, drop, width, gap);
    let rows: Vec<Vec<Line>> = rows
        .iter()
        .map(|row| {
            kept.iter()
                .map(|&i| row.get(i).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    let widths = widths(&rows, width, gap * kept.len().saturating_sub(1));
    let gap = " ".repeat(gap);
    rows.iter()
        .map(|row| {
            let cells = row.iter().zip(&widths).enumerate().map(|(i, (cell, w))| {
                fit(cell, *w, aligns.get(kept[i]).copied().unwrap_or_default())
            });
            join(cells, ["", &gap, ""], &Style::default())
        })
        .collect()
}

/// `plain` as text to print: one line per row, its trailing spaces trimmed.
pub fn plain_text(rows: &[Vec<Line>], aligns: &[Align], drop: &[usize], width: usize) -> String {
    plain(rows, aligns, drop, width, GAP)
        .iter()
        .map(|row| format!("{}\n", row.text().trim_end()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<Line> {
        cells.iter().map(|c| Line::plain(*c)).collect()
    }

    #[test]
    fn columns_line_up_and_numbers_can_align_right() {
        let rows = [
            row(&["model", "in"]),
            row(&["crow-2", "$5"]),
            row(&["k", "$0.25"]),
        ];
        let text: Vec<String> = plain(&rows, &[Align::Left, Align::Right], &[], 40, 2)
            .iter()
            .map(Line::text)
            .collect();
        assert_eq!(text, ["model      in", "crow-2     $5", "k       $0.25"]);
    }

    #[test]
    fn too_wide_shrinks_the_widest_column_first() {
        let rows = [row(&["a fairly long description", "id"])];
        let widths = widths(&rows, 20, 2);
        assert_eq!(widths, [16, 2]);
        let line = &plain(&rows, &[], &[], 20, 2)[0];
        assert_eq!(line.width(), 20);
        assert!(
            line.text().starts_with("a fairly long d…  id"),
            "{}",
            line.text()
        );
    }

    #[test]
    fn narrow_tables_leave_out_columns_in_drop_order_before_cutting() {
        let rows = [row(&["crow-2", "GLM 5.3 Zero Refusal", "$5"])];
        let text = plain(&rows, &[], &[1], 14, 2)[0].text();
        assert_eq!(text, "crow-2  $5");
        // Wide enough for everything: nothing is left out.
        assert_eq!(keep(&rows, &[1], 80, 2), [0, 1, 2]);
    }

    #[test]
    fn a_grid_too_wide_even_cut_leaves_out_its_rightmost_columns() {
        let cells = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let rows = vec![row(&cells), row(&cells)];
        for width in [8, 12, 20, 40] {
            let lines = grid(&rows, &[], width);
            assert!(lines.iter().all(|l| l.width() <= width), "{width}");
            assert!(
                lines[1].text().starts_with("│ a"),
                "{width}: {}",
                lines[1].text()
            );
        }
        assert_eq!(grid(&rows, &[], 40)[1].text().matches('│').count(), 9);
    }

    #[test]
    fn columns_stop_shrinking_at_a_readable_minimum() {
        let rows = [row(&["abcdef", "ghijkl"])];
        assert_eq!(widths(&rows, 2, 0), [3, 3]);
    }
}
