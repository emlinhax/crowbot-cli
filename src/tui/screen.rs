//! Turns frames into terminal bytes. Finished output is committed once, above the live region,
//! and the terminal scrolls it into its own scrollback; only the live region is ever redrawn,
//! and only the lines that changed (pi's line diff, applied to far fewer lines). The live region
//! always ends on the bottom row: until output fills the screen, blank rows pad above it.

use std::fmt::Write as _;

use crate::text::styled::Line;
use crate::text::theme::{Depth, Theme};

const SYNC_START: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";
const CLEAR_LINE: &str = "\x1b[2K";
const CLEAR_BELOW: &str = "\x1b[J";

pub struct Screen {
    width: usize,
    height: usize,
    /// The live lines as last drawn, already rendered to ANSI.
    live: Vec<String>,
    /// The live row the terminal cursor rests on (always the last one after a frame).
    cursor: usize,
    /// The next frame starts from a cleared screen (first frame, after a resize).
    fresh: bool,
    /// Screen rows above the live region holding real output: whatever the shell showed before
    /// crowbot started, then committed lines. The rest of the screen is padding.
    used: usize,
    theme: &'static Theme,
    depth: Depth,
}

impl Screen {
    /// `start_row` is the screen row crowbot starts drawing on.
    pub fn new(
        width: usize,
        height: usize,
        start_row: usize,
        theme: &'static Theme,
        depth: Depth,
    ) -> Self {
        Self {
            width,
            height,
            live: Vec::new(),
            cursor: 0,
            fresh: false,
            used: start_row.min(height),
            theme,
            depth,
        }
    }

    /// The most lines the live region may use; the rest of the screen shows committed output.
    pub fn live_cap(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// CEILING: after a resize the visible screen is cleared and the live region redrawn at the
    /// bottom; committed lines stay in scrollback at their old width. Reflowing them needs pi's full
    /// replay from source blocks.
    pub fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.fresh = true;
        }
    }

    /// The bytes that commit `commit` to scrollback and show `live` below it. Empty when nothing
    /// changed.
    pub fn frame(&mut self, commit: &[Line], live: &[Line]) -> String {
        let cap = self.live_cap();
        let start = live.len().saturating_sub(cap);
        let mut out = String::new();
        if self.fresh {
            out.push_str("\x1b[2J\x1b[H");
            self.live.clear();
            self.cursor = 0;
            self.used = 0;
        }
        self.used = (self.used + commit.len()).min(self.height);
        let pad = self.height.saturating_sub(self.used + live.len() - start);
        let live: Vec<String> = std::iter::repeat_n(String::new(), pad)
            .chain(live[start..].iter().map(|l| self.ansi(l)))
            .collect();

        if self.fresh {
            self.fresh = false;
            self.write_lines(&mut out, commit, &live);
        } else if !commit.is_empty() {
            self.move_to_row(&mut out, 0);
            out.push('\r');
            out.push_str(CLEAR_BELOW);
            self.write_lines(&mut out, commit, &live);
        } else {
            self.patch(&mut out, &live);
        }
        self.live = live;
        if out.is_empty() {
            return out;
        }
        format!("{SYNC_START}{out}{SYNC_END}")
    }

    /// Everything from the cursor down: committed lines, then the whole live region.
    fn write_lines(&mut self, out: &mut String, commit: &[Line], live: &[String]) {
        for line in commit {
            let _ = write!(out, "{}\r\n", self.ansi(line));
        }
        for (i, line) in live.iter().enumerate() {
            if i > 0 {
                out.push_str("\r\n");
            }
            out.push_str(line);
        }
        self.cursor = live.len().saturating_sub(1);
    }

    /// Rewrites only the live lines that changed.
    fn patch(&mut self, out: &mut String, live: &[String]) {
        let first = self
            .live
            .iter()
            .zip(live)
            .position(|(old, new)| old != new)
            .unwrap_or(self.live.len().min(live.len()));
        if first == live.len() && live.len() == self.live.len() {
            return;
        }
        if live.is_empty() {
            self.move_to_row(out, 0);
            out.push('\r');
            out.push_str(CLEAR_BELOW);
            self.cursor = 0;
            return;
        }
        // Only lines removed: redraw from the new last line so the cursor lands there.
        let first = first.min(live.len() - 1);
        self.move_to_row(out, first.min(self.live.len().saturating_sub(1)));
        // Starting past the old region means stepping down onto a new row first.
        let below_old = !self.live.is_empty() && first >= self.live.len();
        for (i, line) in live.iter().enumerate().skip(first) {
            if i > first || below_old {
                out.push_str("\r\n");
            }
            let _ = write!(out, "\r{CLEAR_LINE}{line}");
        }
        if live.len() < self.live.len() {
            out.push_str(CLEAR_BELOW);
        }
        self.cursor = live.len() - 1;
    }

    fn move_to_row(&mut self, out: &mut String, row: usize) {
        match row.cmp(&self.cursor) {
            std::cmp::Ordering::Less => {
                let _ = write!(out, "\x1b[{}A", self.cursor - row);
            }
            std::cmp::Ordering::Greater => {
                let _ = write!(out, "\x1b[{}B", row - self.cursor);
            }
            std::cmp::Ordering::Equal => {}
        }
        self.cursor = row;
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
            Self {
                vt: vt100::Parser::new(ROWS, COLS, 1000),
                screen: Screen::new(COLS as usize, ROWS as usize, 0, theme::get(), Depth::None),
            }
        }

        fn frame(&mut self, commit: &[&str], live: &[&str]) -> String {
            let commit: Vec<Line> = commit.iter().map(|s| Line::plain(*s)).collect();
            let live: Vec<Line> = live.iter().map(|s| Line::plain(*s)).collect();
            let bytes = self.screen.frame(&commit, &live);
            self.vt.process(bytes.as_bytes());
            bytes
        }

        /// Scrollback then screen, top to bottom, with the blank padding rows dropped.
        fn history(&mut self) -> Vec<String> {
            let screen = self.vt.screen_mut();
            screen.set_scrollback(usize::MAX);
            let depth = screen.scrollback();
            let mut lines = Vec::new();
            for offset in (1..=depth).rev() {
                screen.set_scrollback(offset);
                lines.push(screen.rows(0, COLS).next().unwrap_or_default());
            }
            screen.set_scrollback(0);
            lines.extend(screen.rows(0, COLS));
            lines
                .into_iter()
                .map(|l| l.trim_end().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        }
    }

    #[test]
    fn committed_lines_land_in_scrollback_exactly_once() {
        let mut t = Term::new();
        t.frame(&[], &["editor", "footer"]);
        let mut expected = Vec::new();
        for batch in 0..10 {
            let lines: Vec<String> = (0..3).map(|i| format!("line {batch}.{i}")).collect();
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            t.frame(&refs, &["editor", "footer"]);
            expected.extend(lines);
        }
        expected.extend(["editor".to_owned(), "footer".to_owned()]);
        assert_eq!(t.history(), expected);
    }

    #[test]
    fn the_live_region_grows_and_shrinks_without_leftovers() {
        let mut t = Term::new();
        t.frame(&["done"], &["a", "b", "c", "d", "e"]);
        t.frame(&[], &["a", "x"]);
        assert_eq!(t.history(), vec!["done", "a", "x"]);
        t.frame(&[], &["a", "x", "y", "z"]);
        assert_eq!(t.history(), vec!["done", "a", "x", "y", "z"]);
        t.frame(&["more"], &["only"]);
        assert_eq!(t.history(), vec!["done", "more", "only"]);
    }

    #[test]
    fn unchanged_frames_write_nothing_and_changes_write_only_their_lines() {
        let mut t = Term::new();
        t.frame(&[], &["spinner 1", "editor", "footer"]);
        assert!(t.frame(&[], &["spinner 1", "editor", "footer"]).is_empty());
        let bytes = t.frame(&[], &["spinner 1", "editor!", "footer"]);
        assert!(bytes.contains("editor!"));
        assert!(!bytes.contains("spinner"));
        assert!(bytes.starts_with(SYNC_START) && bytes.ends_with(SYNC_END));
        assert_eq!(t.history(), vec!["spinner 1", "editor!", "footer"]);
    }

    #[test]
    fn nothing_is_drawn_wider_than_the_terminal() {
        let mut t = Term::new();
        let long = "x".repeat(100);
        t.frame(&[&long], &[&long]);
        for line in t.history() {
            assert!(line.chars().count() <= COLS as usize, "{line}");
        }
        assert_eq!(t.history().len(), 2);
    }

    #[test]
    fn the_live_region_never_exceeds_the_screen() {
        let mut t = Term::new();
        let tall: Vec<String> = (0..20).map(|i| format!("row {i}")).collect();
        let refs: Vec<&str> = tall.iter().map(String::as_str).collect();
        t.frame(&[], &refs);
        let shown = t.history();
        assert_eq!(shown.len(), ROWS as usize - 1);
        assert_eq!(shown.last().unwrap(), "row 19");
    }

    #[test]
    fn random_frame_sequences_keep_history_exact() {
        let mut rng = fastrand::Rng::with_seed(11);
        for _ in 0..300 {
            let mut t = Term::new();
            let mut committed: Vec<String> = Vec::new();
            let mut live: Vec<String> = Vec::new();
            for step in 0..rng.usize(1..12) {
                let commit: Vec<String> = (0..rng.usize(0..3))
                    .map(|i| format!("c{step}.{i}"))
                    .collect();
                live = (0..rng.usize(1..6))
                    .map(|i| format!("l{}.{i}", rng.usize(0..3)))
                    .collect();
                let c: Vec<&str> = commit.iter().map(String::as_str).collect();
                let l: Vec<&str> = live.iter().map(String::as_str).collect();
                t.frame(&c, &l);
                committed.extend(commit);
            }
            let mut expected = committed.clone();
            expected.extend(live.clone());
            assert_eq!(t.history(), expected);
        }
    }

    #[test]
    fn a_resize_redraws_the_live_region_cleanly() {
        let mut t = Term::new();
        t.frame(&["kept"], &["a", "b"]);
        t.screen.resize(20, ROWS as usize);
        t.frame(&[], &["a", "b"]);
        let shown = rows(&t);
        assert_eq!(&shown[ROWS as usize - 2..], ["a", "b"]);
    }

    /// The visible rows, top to bottom, blank ones included.
    fn rows(t: &Term) -> Vec<String> {
        t.vt.screen()
            .rows(0, COLS)
            .map(|r| r.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn the_live_region_sits_on_the_bottom_row_from_the_first_frame() {
        let mut t = Term::new();
        t.frame(&["welcome"], &["editor", "footer"]);
        let shown = rows(&t);
        assert_eq!(shown[0], "welcome");
        assert_eq!(&shown[ROWS as usize - 2..], ["editor", "footer"]);
        // Output fills the gap from the top; the editor stays on the bottom.
        t.frame(&["one", "two"], &["editor", "status", "footer"]);
        let shown = rows(&t);
        assert_eq!(&shown[..3], ["welcome", "one", "two"]);
        assert_eq!(shown.last().unwrap(), "footer");
    }

    #[test]
    fn starting_mid_screen_keeps_what_the_shell_showed() {
        let mut t = Term::new();
        t.vt.process(b"$ crowbot\r\n");
        t.screen = Screen::new(COLS as usize, ROWS as usize, 1, theme::get(), Depth::None);
        t.frame(&[], &["editor", "footer"]);
        let shown = rows(&t);
        assert_eq!(shown[0], "$ crowbot");
        assert_eq!(shown.last().unwrap(), "footer");
        assert_eq!(shown.len(), ROWS as usize);
    }
}
