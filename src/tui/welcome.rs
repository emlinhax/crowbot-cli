//! The first thing on screen: crowbot's raven beside who and where you are.

use crate::text::styled::{Line, Style, width};
use crate::tui::ui;

const RAVEN: &str = include_str!("../../data/raven.txt");
/// The same bird for consoles without a braille font.
const RAVEN_ASCII: &str = include_str!("../../data/raven_ascii.txt");

/// Columns between the raven and the text beside it.
const GUTTER: usize = 3;
/// Narrowest text column worth putting beside the raven rather than under it.
const MIN_TEXT: usize = 28;

/// Kept by the transcript, which redraws the welcome at every width.
#[derive(Clone)]
pub struct Info {
    pub version: &'static str,
    pub cwd: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode_label: String,
    pub mode_color: String,
    pub logged_in: bool,
    /// Braille glyphs render (Windows Terminal, any Unix terminal); legacy conhost does not.
    pub braille: bool,
}

pub fn render(info: &Info, width: usize) -> Vec<Line> {
    let art = if info.braille { RAVEN } else { RAVEN_ASCII };
    let raven: Vec<&str> = art.lines().collect();
    let raven_width = raven.iter().map(|l| self::width(l)).max().unwrap_or(0);
    // Every row as wide as the widest, so text beside the bird lines up.
    let bird =
        |row: &str| Line::styled(row, Style::fg("accent")).padded(raven_width, &Style::default());
    let text = text(info);
    let mut out = Vec::new();

    if width < raven_width {
        out.extend(text);
    } else if width >= raven_width + GUTTER + MIN_TEXT {
        // Text sits beside the raven, vertically centred on it.
        let top = raven.len().saturating_sub(text.len()) / 2;
        let avail = width - raven_width - GUTTER;
        for (i, row) in raven.iter().enumerate() {
            let mut line = bird(row);
            if let Some(t) = i.checked_sub(top).and_then(|j| text.get(j)) {
                line.push(" ".repeat(GUTTER), Style::default());
                line.extend(t.truncate(avail));
            }
            out.push(line);
        }
    } else {
        out.extend(raven.iter().map(|r| bird(r)));
        out.push(Line::default());
        out.extend(text);
    }
    out.push(Line::default());
    out.into_iter().map(|l| l.truncate(width)).collect()
}

fn text(info: &Info) -> Vec<Line> {
    let ui = ui::get();
    let mut name = Line::styled("crowbot", Style::fg("accent").bold());
    name.push(format!(" v{}", info.version), Style::fg("muted"));

    let mut model = Line::plain(&info.model);
    if let Some(effort) = &info.effort {
        model.push(format!(" · {effort}"), Style::fg("muted"));
    }
    model.push(" · ", Style::fg("muted"));
    model.push(&info.mode_label, Style::fg(&info.mode_color).bold());

    let mut lines = vec![
        name,
        Line::styled(&info.cwd, Style::fg("muted")),
        model,
        Line::default(),
    ];
    if !info.logged_in {
        lines.push(Line::styled(&ui.text.not_logged_in, Style::fg("warn")));
    }
    lines.extend(ui.tips.iter().map(|t| Line::styled(t, Style::fg("muted"))));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(braille: bool) -> Info {
        Info {
            version: "0.1.0",
            cwd: "~/src/proj".into(),
            model: "crow-2".into(),
            effort: Some("high".into()),
            mode_label: "MANUAL".into(),
            mode_color: "mode_manual".into(),
            logged_in: true,
            braille,
        }
    }

    fn snapshot(lines: &[Line]) -> String {
        lines
            .iter()
            .map(Line::to_tagged)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_raven_is_thirteen_rows_of_braille() {
        let rows: Vec<&str> = RAVEN.lines().collect();
        assert_eq!(rows.len(), 13);
        for row in rows {
            assert_eq!(row.chars().count(), 23);
            assert!(row.chars().all(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
        }
    }

    #[test]
    fn the_ascii_raven_is_the_same_size_in_plain_characters() {
        let rows: Vec<&str> = RAVEN_ASCII.lines().collect();
        assert_eq!(rows.len(), RAVEN.lines().count());
        for row in rows {
            assert!(row.chars().count() <= 23, "{row:?}");
            assert!(
                row.chars().all(|c| c == ' ' || c.is_ascii_graphic()),
                "{row:?}"
            );
        }
    }

    #[test]
    fn layouts_by_width() {
        insta::assert_snapshot!("welcome_wide", snapshot(&render(&info(true), 100)));
        insta::assert_snapshot!("welcome_narrow", snapshot(&render(&info(true), 40)));
        insta::assert_snapshot!("welcome_no_braille", snapshot(&render(&info(false), 100)));
    }

    #[test]
    fn nothing_overflows() {
        for w in [10, 24, 40, 60, 100] {
            for line in render(&info(true), w) {
                assert!(line.width() <= w);
            }
        }
    }
}
