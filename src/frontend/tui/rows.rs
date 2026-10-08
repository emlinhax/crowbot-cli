//! A list inside a card: the selection mark and its scroll window, numbered option rows, how a
//! selection moves, and a cap on a long body. Every card with a list draws and moves it here.

use crate::config::limits;
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::ui;
use crate::io::term::{KeyCode, KeyEvent};
use crate::text::styled::{Line, Style};

/// The selection mark's width: `› `, or two spaces on the other rows.
pub const MARK: usize = 2;

/// `rows` behind the selection mark, scrolled to keep the selected one in view; the chosen row
/// takes the accent whole.
pub fn marked(rows: impl IntoIterator<Item = Line>, selected: usize) -> Vec<Line> {
    let visible = limits::get().tui.palette_rows.value.get();
    let start = (selected + 1).saturating_sub(visible);
    rows.into_iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, row)| {
            if i == selected {
                let chosen = chosen();
                let mark = format!("{} ", ui::get().marks.selected);
                let mut line = Line::styled(mark, chosen.clone());
                line.push(row.text(), chosen);
                line
            } else {
                let mut line = Line::plain(" ".repeat(MARK));
                line.extend(row);
                line
            }
        })
        .collect()
}

/// A numbered option, ` › 1 [x] label`: the mark when `chosen`, and `tick` for a multiple choice.
pub fn option(n: usize, label: &str, tick: &str, chosen: bool) -> Line {
    let (mark, style) = if chosen {
        (ui::get().marks.selected.as_str(), self::chosen())
    } else {
        (" ", Style::default())
    };
    Line::styled(format!(" {mark} {n} {tick}{label}"), style)
}

/// Where the selection goes on `action` in a list of `len`: Up and Down move it, wrapping round
/// the ends when `wrap`, else stopping there; anything else leaves it.
pub fn step(selected: usize, action: Action, len: usize, wrap: bool) -> usize {
    let last = len.saturating_sub(1);
    match (action, wrap) {
        (Action::Up, true) => selected.checked_sub(1).unwrap_or(last),
        (Action::Up, false) => selected.saturating_sub(1),
        (Action::Down, true) if selected >= last => 0,
        (Action::Down, _) => (selected + 1).min(last),
        _ => selected,
    }
}

/// The row a digit key picks in a list of `len`, counting from 1.
pub fn digit(key: &KeyEvent, len: usize) -> Option<usize> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let n = c.to_digit(10)? as usize;
    (1..=len).contains(&n).then(|| n - 1)
}

/// At most `max` of `lines`; a last line says how many were left out.
pub fn capped(mut lines: Vec<Line>, max: usize) -> Vec<Line> {
    if lines.len() > max {
        let rest = lines.len() - max;
        lines.truncate(max);
        lines.push(Line::styled(
            ui::get().text.more_lines.of(rest),
            Style::fg("muted"),
        ));
    }
    lines
}

fn chosen() -> Style {
    Style::fg("accent").bold()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_says_how_many_lines_it_left_out() {
        let lines = |n: usize| {
            (0..n)
                .map(|i| Line::plain(i.to_string()))
                .collect::<Vec<_>>()
        };
        let texts = |ls: Vec<Line>| ls.iter().map(Line::text).collect::<Vec<_>>();
        assert_eq!(texts(capped(lines(3), 2)), vec!["0", "1", "… 1 more line"]);
        assert_eq!(texts(capped(lines(5), 2)), vec!["0", "1", "… 3 more lines"]);
        assert_eq!(texts(capped(lines(2), 2)), vec!["0", "1"]);
    }

    #[test]
    fn a_selection_stops_at_the_ends_or_wraps() {
        assert_eq!(step(0, Action::Up, 3, false), 0);
        assert_eq!(step(2, Action::Down, 3, false), 2);
        assert_eq!(step(0, Action::Up, 3, true), 2);
        assert_eq!(step(2, Action::Down, 3, true), 0);
        assert_eq!(step(1, Action::Submit, 3, true), 1);
    }
}
