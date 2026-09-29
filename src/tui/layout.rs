//! One frame of the whole screen: the transcript in the rows the bar leaves, one blank row that
//! keeps it off the bar (or says how much is hidden below), then the bar stack on the bottom. A
//! floating card is drawn over the rows just above the bar instead of taking room from the
//! transcript, so opening one moves nothing.

use std::time::Instant;

use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::tui::feed::Feed;
use crate::tui::ui;
use crate::tui::view::View;

pub struct Frame {
    pub rows: Vec<Line>,
    /// The transcript block drawn on each row, for clicks.
    pub blocks: Vec<Option<usize>>,
}

pub fn compose(
    feed: &mut Feed,
    view: &mut View,
    stack: &[Line],
    overlay: &[Line],
    (width, height): (usize, usize),
    now: Instant,
) -> Frame {
    // A terminal too short for the whole bar keeps its bottom, where the editor is.
    let stack = &stack[stack.len().saturating_sub(height)..];
    let area = height.saturating_sub(stack.len() + 1);
    let total = feed.measure(width, now);
    let top = view.top(total, area);
    let (mut rows, mut blocks): (Vec<Line>, Vec<Option<usize>>) =
        feed.rows(top, top + area).into_iter().unzip();
    rows.resize(area, Line::default());
    blocks.resize(area, None);
    if height > stack.len() {
        let below = total.saturating_sub(top + area);
        rows.push(if below > 0 {
            let text = fill(&ui::get().text.more_below, &[("n", &below.to_string())]);
            Line::styled(text, Style::fg("muted")).truncate(width)
        } else {
            Line::default()
        });
        blocks.push(None);
    }
    // Too tall for the room above the bar: its bottom, nearest the editor, stays.
    let covered = overlay.len().min(rows.len());
    let first = rows.len() - covered;
    for (i, line) in overlay[overlay.len() - covered..].iter().enumerate() {
        rows[first + i] = line.clone();
        blocks[first + i] = None;
    }
    rows.extend(stack.iter().cloned());
    blocks.resize(rows.len(), None);
    Frame { rows, blocks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::feed::Block;

    fn feed(lines: usize) -> Feed {
        let mut feed = Feed::new();
        for i in 0..lines {
            feed.push(Block::Notice {
                text: format!("n{i}"),
                role: "muted".into(),
            });
        }
        feed
    }

    fn texts(frame: &Frame) -> Vec<String> {
        frame.rows.iter().map(Line::text).collect()
    }

    #[test]
    fn the_bar_is_on_the_bottom_with_a_blank_row_above_it() {
        let now = crate::io::clock::instant();
        let bar = [
            Line::plain("── MANUAL"),
            Line::plain("› "),
            Line::plain("──"),
        ];
        let frame = compose(&mut feed(1), &mut View::default(), &bar, &[], (40, 10), now);
        let rows = texts(&frame);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], "n0");
        assert_eq!(&rows[6..], ["", "── MANUAL", "› ", "──"]);
        assert_eq!(frame.blocks[0], Some(0));
        assert_eq!(frame.blocks[9], None);
    }

    #[test]
    fn a_long_transcript_shows_its_end_and_says_what_is_hidden_when_scrolled() {
        let now = crate::io::clock::instant();
        let bar = [Line::plain("bar")];
        let mut view = View::default();
        let mut long = feed(20);
        let rows = texts(&compose(&mut long, &mut view, &bar, &[], (40, 10), now));
        // 20 notices one blank apart are 39 rows; the last 8 fit.
        assert_eq!(rows[7], "n19");
        assert_eq!(rows[8], "");
        view.scroll(-4);
        let rows = texts(&compose(&mut long, &mut view, &bar, &[], (40, 10), now));
        assert_eq!(rows[8], "↓ 4 more · PgDn");
        assert_eq!(rows[9], "bar");
    }

    #[test]
    fn a_tiny_terminal_keeps_the_bottom_of_the_bar() {
        let now = crate::io::clock::instant();
        let bar = [
            Line::plain("status"),
            Line::plain("editor"),
            Line::plain("rule"),
        ];
        let frame = compose(&mut feed(3), &mut View::default(), &bar, &[], (40, 2), now);
        assert_eq!(texts(&frame), ["editor", "rule"]);
    }

    #[test]
    fn a_floating_card_covers_the_rows_above_the_bar_and_moves_nothing() {
        let now = crate::io::clock::instant();
        let bar = [Line::plain("bar")];
        let mut view = View::default();
        let mut long = feed(20);
        let before = texts(&compose(&mut long, &mut view, &bar, &[], (40, 10), now));
        let popup = [Line::plain("╭ commands"), Line::plain("╰──")];
        let frame = compose(&mut long, &mut view, &bar, &popup, (40, 10), now);
        let after = texts(&frame);
        // Everything above the card is exactly where it was.
        assert_eq!(after[..7], before[..7]);
        assert_eq!(&after[7..], ["╭ commands", "╰──", "bar"]);
        assert_eq!(
            frame.blocks[7], None,
            "clicks on the card toggle nothing beneath"
        );
        // Taller than the room: its bottom, by the editor, is what shows.
        let tall: Vec<Line> = (0..20).map(|i| Line::plain(format!("c{i}"))).collect();
        let rows = texts(&compose(&mut long, &mut view, &bar, &tall, (40, 10), now));
        assert_eq!(rows[0], "c11");
        assert_eq!(rows[8], "c19");
    }
}
