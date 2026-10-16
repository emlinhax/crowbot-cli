//! Turns frames into terminal bytes. A frame is the whole screen, one line per row; only the
//! rows that changed since the last frame are rewritten, each at an absolute position, so
//! nothing a frame draws can shift another row.

use std::fmt::Write as _;

use crate::io::term::Depth;
use crate::text::controls;
use crate::text::styled::{Line, Style};
use crate::text::theme::{Rgb, Theme};

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
        to_ansi(&line.truncate(self.width), self.theme, self.depth)
    }
}

/// SGR per span, closed with a reset so nothing leaks to the next line.
fn to_ansi(line: &Line, theme: &Theme, depth: Depth) -> String {
    let mut out = String::new();
    for span in &line.spans {
        let sgr = sgr(&span.style, theme, depth);
        if let Some(url) = &span.style.link {
            let _ = write!(out, "\x1b]8;;{}\x1b\\", controls::osc_url(url));
        }
        if sgr.is_empty() {
            out.push_str(&span.text);
        } else {
            let _ = write!(out, "\x1b[{sgr}m{}\x1b[0m", span.text);
        }
        if span.style.link.is_some() {
            out.push_str("\x1b]8;;\x1b\\");
        }
    }
    out
}

fn sgr(style: &Style, theme: &Theme, depth: Depth) -> String {
    let mut codes: Vec<String> = Vec::new();
    for (on, code) in [
        (style.bold, "1"),
        (style.dim, "2"),
        (style.italic, "3"),
        (style.underline, "4"),
        (style.reverse, "7"),
        (style.strike, "9"),
    ] {
        if on {
            codes.push(code.into());
        }
    }
    for (role, base) in [(&style.fg, 38), (&style.bg, 48)] {
        let Some(color) = role.as_deref().and_then(|r| theme.color(r)) else {
            continue;
        };
        match depth {
            Depth::TrueColor => codes.push(format!("{base};2;{};{};{}", color.0, color.1, color.2)),
            Depth::Ansi256 => codes.push(format!("{base};5;{}", to_256(color))),
            Depth::None => {}
        }
    }
    codes.join(";")
}

/// The nearest xterm-256 colour, for terminals without truecolour.
pub fn to_256(Rgb(r, g, b): Rgb) -> u8 {
    let level = |v: u8| -> u8 {
        match v {
            0..=47 => 0,
            48..=114 => 1,
            _ => (v - 35) / 40,
        }
    };
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * lr + 6 * lg + lb;
    let step = |l: u8| if l == 0 { 0 } else { 55 + 40 * l as u16 };
    let cube_err = [(r, lr), (g, lg), (b, lb)]
        .iter()
        .map(|(v, l)| (i32::from(*v) - i32::from(step(*l))).pow(2))
        .sum::<i32>();
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let gray_index = if avg > 238 {
        23
    } else {
        avg.saturating_sub(3) / 10
    } as u8;
    let gray = 8 + 10 * i32::from(gray_index);
    let gray_err = [r, g, b]
        .iter()
        .map(|v| (i32::from(*v) - gray).pow(2))
        .sum::<i32>();
    if gray_err < cube_err {
        232 + gray_index
    } else {
        cube
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
    fn a_link_cannot_put_escapes_on_the_terminal() {
        let link = Style {
            link: Some("https://x.dev/\u{1b}\\\u{1b}]0;pwned\u{7}".into()),
            ..Style::default()
        };
        let ansi = to_ansi(&Line::styled("x", link), theme::get(), Depth::None);
        assert_eq!(ansi.matches('\u{1b}').count(), 4, "{ansi:?}");
    }

    #[test]
    fn ansi_resets_after_every_styled_span() {
        let line = Line::styled("ok", Style::fg("accent").bold());
        let ansi = to_ansi(&line, theme::get(), Depth::TrueColor);
        assert_eq!(ansi, "\x1b[1;38;2;0;220;130mok\x1b[0m");
        assert_eq!(
            to_ansi(&line, theme::get(), Depth::None),
            "\x1b[1mok\x1b[0m"
        );
        assert_eq!(
            to_ansi(&line, theme::get(), Depth::Ansi256),
            "\x1b[1;38;5;42mok\x1b[0m"
        );
    }
    #[test]
    fn nearest_256_colours() {
        assert_eq!(to_256(Rgb(0, 0, 0)), 16);
        assert_eq!(to_256(Rgb(255, 255, 255)), 231);
        assert_eq!(to_256(Rgb(128, 128, 128)), 244);
        assert_eq!(to_256(Rgb(0, 220, 130)), 42);
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
