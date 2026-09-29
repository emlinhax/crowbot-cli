//! Turns frames into terminal bytes. A frame is the whole screen, one line per row; only the
//! rows that changed since the last frame are rewritten, each at an absolute position, so
//! nothing a frame draws can shift another row.

use std::fmt::Write as _;

use crate::text::styled::Line;
use crate::text::theme::{Depth, Theme};

const SYNC_START: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";
const CLEAR_SCREEN: &str = "\x1b[2J";
const CLEAR_LINE: &str = "\x1b[2K";

pub struct Screen {
    width: usize,
    height: usize,
    /// The rows as last drawn, already rendered to ANSI; empty until the first frame.
    rows: Vec<String>,
    theme: &'static Theme,
    depth: Depth,
}

impl Screen {
    pub fn new(width: usize, height: usize, theme: &'static Theme, depth: Depth) -> Self {
        Self {
            width,
            height,
            rows: Vec::new(),
            theme,
            depth,
        }
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.rows.clear();
        }
    }

    /// The bytes that show `lines` as the screen, top row first; missing rows are blank and
    /// extra ones are dropped. Empty when nothing changed.
    pub fn frame(&mut self, lines: &[Line]) -> String {
        let rows: Vec<String> = (0..self.height)
            .map(|i| lines.get(i).map(|l| self.ansi(l)).unwrap_or_default())
            .collect();
        let mut out = String::new();
        let fresh = self.rows.len() != rows.len();
        if fresh {
            out.push_str(CLEAR_SCREEN);
        }
        for (i, row) in rows.iter().enumerate() {
            if !fresh && self.rows[i] == *row {
                continue;
            }
            let _ = write!(out, "\x1b[{};1H{CLEAR_LINE}{row}", i + 1);
        }
        self.rows = rows;
        if out.is_empty() {
            return out;
        }
        format!("{SYNC_START}{out}{SYNC_END}")
    }

    /// One line as ANSI, never wider than the terminal.
    fn ansi(&self, line: &Line) -> String {
        line.truncate(self.width).to_ansi(self.theme, self.depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::styled::Line;
    use crate::text::theme;

    const ROWS: u16 = 8;
    const COLS: u16 = 30;

    struct Term {
        vt: vt100::Parser,
        screen: Screen,
    }

    impl Term {
        fn new() -> Self {
            let mut vt = vt100::Parser::new(ROWS, COLS, 0);
            // What the session's terminal guard sets: no auto-wrap.
            vt.process(b"\x1b[?7l");
            Self {
                vt,
                screen: Screen::new(COLS as usize, ROWS as usize, theme::get(), Depth::None),
            }
        }

        fn frame(&mut self, lines: &[&str]) -> String {
            let lines: Vec<Line> = lines.iter().map(|s| Line::plain(*s)).collect();
            let bytes = self.screen.frame(&lines);
            self.vt.process(bytes.as_bytes());
            bytes
        }

        /// The visible rows, top to bottom, blank ones included.
        fn rows(&self) -> Vec<String> {
            self.vt
                .screen()
                .rows(0, COLS)
                .map(|r| r.trim_end().to_owned())
                .collect()
        }
    }

    /// `n` rows ending in the message bar, as the session composes them.
    fn with_bar(body: &[&str]) -> Vec<String> {
        let mut rows: Vec<String> = body.iter().map(|s| (*s).to_owned()).collect();
        rows.resize(ROWS as usize - 1, String::new());
        rows.push("bar".into());
        rows
    }

    #[test]
    fn the_screen_shows_exactly_the_frame() {
        let mut t = Term::new();
        t.frame(&["one", "two"]);
        let rows = t.rows();
        assert_eq!(rows.len(), ROWS as usize);
        assert_eq!(&rows[..3], ["one", "two", ""]);
    }

    #[test]
    fn unchanged_frames_write_nothing_and_changes_write_only_their_rows() {
        let mut t = Term::new();
        t.frame(&["spinner 1", "editor", "footer"]);
        assert!(t.frame(&["spinner 1", "editor", "footer"]).is_empty());
        let bytes = t.frame(&["spinner 1", "editor!", "footer"]);
        assert!(bytes.contains("editor!"));
        assert!(!bytes.contains("spinner") && !bytes.contains("footer"));
        assert!(bytes.starts_with(SYNC_START) && bytes.ends_with(SYNC_END));
        assert_eq!(&t.rows()[..3], ["spinner 1", "editor!", "footer"]);
    }

    #[test]
    fn nothing_is_drawn_wider_than_the_terminal() {
        let mut t = Term::new();
        let long = "x".repeat(100);
        t.frame(&[&long, "next"]);
        let rows = t.rows();
        assert_eq!(rows[0].chars().count(), COLS as usize);
        assert_eq!(rows[1], "next");
    }

    #[test]
    fn a_resize_redraws_everything() {
        let mut t = Term::new();
        t.frame(&["kept", "b"]);
        t.vt.process(b"\x1b[2J");
        t.screen.resize(20, ROWS as usize);
        t.frame(&["kept", "b"]);
        assert_eq!(&t.rows()[..2], ["kept", "b"]);
    }

    #[test]
    fn the_bar_stays_on_the_bottom_row_whatever_grows_and_shrinks() {
        let mut rng = fastrand::Rng::with_seed(11);
        let mut t = Term::new();
        for step in 0..300 {
            let body: Vec<String> = (0..rng.usize(0..12))
                .map(|i| format!("r{step}.{i}"))
                .collect();
            let refs: Vec<&str> = body.iter().map(String::as_str).collect();
            let frame = with_bar(&refs);
            let refs: Vec<&str> = frame.iter().map(String::as_str).collect();
            t.frame(&refs);
            let rows = t.rows();
            assert_eq!(rows.last().unwrap(), "bar", "step {step}: {rows:?}");
            let shown: Vec<&str> = refs[..ROWS as usize - 1].to_vec();
            assert_eq!(&rows[..ROWS as usize - 1], shown.as_slice(), "step {step}");
        }
    }
}
