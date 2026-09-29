//! Columns that fit a width. Each column starts as wide as its widest cell, then the widest one
//! gives up a cell at a time (its cells cut with `…`) until the table fits, so rows always stay
//! aligned. Markdown grids, `crowbot models` and the model picker all lay out here.

use crate::text::styled::{Line, Style};

/// Narrowest a column is shrunk to; a letter and the `…` still say what was there.
const MIN_COLUMN: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Right,
}

/// Column widths for `rows` in `width` cells, `overhead` of which go to borders and gaps.
/// CEILING: cells are cut, never wrapped; wrapping would need rows of several lines.
pub fn widths(rows: &[Vec<Line>], width: usize, overhead: usize) -> Vec<usize> {
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
        if w <= MIN_COLUMN {
            break;
        }
        widths[i] -= 1;
    }
    widths
}

/// The columns to show in `width`: all of them while they fit uncut, else as few left out as
/// needed, in `drop` order. What is kept may still be cut to fit.
pub fn keep(rows: &[Vec<Line>], drop: &[usize], width: usize, gap: usize) -> Vec<usize> {
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
pub fn fit(cell: &Line, width: usize, align: Align) -> Line {
    let cell = cell.truncate(width);
    let pad = " ".repeat(width.saturating_sub(cell.width()));
    match align {
        Align::Left => {
            let mut out = cell;
            out.push(pad, Style::default());
            out
        }
        Align::Right => {
            let mut out = Line::plain(pad);
            out.extend(cell);
            out
        }
    }
}

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
    rows.iter()
        .map(|row| {
            let mut line = Line::default();
            for (i, (cell, w)) in row.iter().zip(&widths).enumerate() {
                if i > 0 {
                    line.push(" ".repeat(gap), Style::default());
                }
                let align = aligns.get(kept[i]).copied().unwrap_or_default();
                line.extend(fit(cell, *w, align));
            }
            line
        })
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
    fn columns_stop_shrinking_at_a_readable_minimum() {
        let rows = [row(&["abcdef", "ghijkl"])];
        assert_eq!(widths(&rows, 2, 0), [3, 3]);
    }
}
