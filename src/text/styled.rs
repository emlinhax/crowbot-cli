//! Styled text as the TUI draws it: lines of spans, each span naming theme roles rather than
//! colours. Width is measured in terminal cells, per grapheme.

use std::fmt::Write as _;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::controls;
use crate::text::theme::{self, Depth, Theme};

/// Tabs become this many spaces so every cell is accounted for.
pub const TAB: &str = "    ";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// A role in data/theme.toml; `None` is the terminal default.
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub reverse: bool,
    /// Makes the span an OSC 8 hyperlink.
    pub link: Option<String>,
}

impl Style {
    pub fn fg(role: &str) -> Self {
        Self {
            fg: Some(role.to_owned()),
            ..Self::default()
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    /// The same, on a background role.
    pub fn on(mut self, role: &str) -> Self {
        self.bg = Some(role.to_owned());
        self
    }

    /// `self`, with anything `over` sets taking precedence.
    pub fn patch(&self, over: &Style) -> Style {
        Style {
            fg: over.fg.clone().or_else(|| self.fg.clone()),
            bg: over.bg.clone().or_else(|| self.bg.clone()),
            bold: self.bold || over.bold,
            dim: self.dim || over.dim,
            italic: self.italic || over.italic,
            underline: self.underline || over.underline,
            strike: self.strike || over.strike,
            reverse: self.reverse || over.reverse,
            link: over.link.clone().or_else(|| self.link.clone()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(text, Style::default())
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        let mut line = Self::default();
        line.push(text, style);
        line
    }

    pub fn push(&mut self, text: impl Into<String>, style: Style) {
        let mut text: String = text.into().replace('\t', TAB);
        // Spans hold text from anywhere; only rendering adds escapes.
        if text.chars().any(char::is_control) {
            text = controls::for_terminal(&text).into_owned();
        }
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(&text),
            _ => self.spans.push(Span { text, style }),
        }
    }

    /// Filled with `style` out to `width` cells, so a background reaches the edge.
    pub fn padded(mut self, width: usize, style: &Style) -> Line {
        let room = width.saturating_sub(self.width());
        self.push(" ".repeat(room), style.clone());
        self
    }

    pub fn extend(&mut self, other: Line) {
        for span in other.spans {
            self.push(span.text, span.style);
        }
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| width(&s.text)).sum()
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Word-wraps to `width` cells; `indent` starts every continuation line.
    pub fn wrap(&self, width: usize, indent: &Line) -> Vec<Line> {
        let width = width.max(1);
        let indent_width = indent.width().min(width.saturating_sub(1));
        let mut out = Vec::new();
        let mut current = Line::default();
        let mut used = 0;
        let mut first = true;
        for (word, style) in words(self) {
            let w = width_of(&word);
            let is_space = word.chars().all(char::is_whitespace);
            if used + w > width && used > (if first { 0 } else { indent_width }) {
                out.push(current.trim_end());
                current = indent.clone();
                used = indent_width;
                first = false;
                if is_space {
                    continue;
                }
            }
            if used + w > width {
                // A word longer than the line breaks at grapheme boundaries.
                for g in word.graphemes(true) {
                    let gw = width_of(g);
                    if used + gw > width && used > indent_width {
                        out.push(current);
                        current = indent.clone();
                        used = indent_width;
                    }
                    current.push(g, style.clone());
                    used += gw;
                }
                continue;
            }
            current.push(word, style);
            used += w;
        }
        out.push(current.trim_end());
        out
    }

    /// Cuts to at most `width` cells, marking the cut with `…`.
    pub fn truncate(&self, width: usize) -> Line {
        if self.width() <= width {
            return self.clone();
        }
        let mut out = Line::default();
        let mut used = 0;
        let budget = width.saturating_sub(1);
        'spans: for span in &self.spans {
            for g in span.text.graphemes(true) {
                let gw = width_of(g);
                if used + gw > budget {
                    break 'spans;
                }
                out.push(g, span.style.clone());
                used += gw;
            }
        }
        if width > 0 {
            out.push("…", Style::fg("muted"));
        }
        out
    }

    fn trim_end(mut self) -> Line {
        while let Some(last) = self.spans.last_mut() {
            let trimmed = last.text.trim_end().len();
            if trimmed == 0 {
                self.spans.pop();
            } else {
                last.text.truncate(trimmed);
                break;
            }
        }
        self
    }

    /// Terminal output: SGR per span, closed with a reset so nothing leaks to the next line.
    pub fn to_ansi(&self, theme: &Theme, depth: Depth) -> String {
        let mut out = String::new();
        for span in &self.spans {
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

    /// `[role+bold]text[/]` markup, for snapshot tests that must not depend on colours.
    #[cfg(test)]
    pub fn to_tagged(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            let s = &span.style;
            let mut tags: Vec<String> = Vec::new();
            if let Some(fg) = &s.fg {
                tags.push(fg.clone());
            }
            if let Some(bg) = &s.bg {
                tags.push(format!("bg:{bg}"));
            }
            for (on, name) in [
                (s.bold, "bold"),
                (s.dim, "dim"),
                (s.italic, "italic"),
                (s.underline, "underline"),
                (s.strike, "strike"),
                (s.reverse, "reverse"),
            ] {
                if on {
                    tags.push(name.into());
                }
            }
            if s.link.is_some() {
                tags.push("link".into());
            }
            if tags.is_empty() {
                out.push_str(&span.text);
            } else {
                let _ = write!(out, "[{}]{}[/]", tags.join("+"), span.text);
            }
        }
        out
    }
}

/// Display width in terminal cells.
pub fn width(text: &str) -> usize {
    text.graphemes(true).map(width_of).sum()
}

fn width_of(grapheme: &str) -> usize {
    if grapheme == "\t" {
        return TAB.len();
    }
    UnicodeWidthStr::width(grapheme)
}

/// Words and the whitespace runs between them, each with its span's style.
fn words(line: &Line) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for span in &line.spans {
        let mut current = String::new();
        let mut in_space = None;
        for g in span.text.graphemes(true) {
            let space = g.chars().all(char::is_whitespace);
            // East Asian wide characters may break anywhere, like spaces.
            let wide = !space && width_of(g) == 2;
            if (in_space.is_some_and(|s| s != space) || wide) && !current.is_empty() {
                out.push((std::mem::take(&mut current), span.style.clone()));
            }
            current.push_str(g);
            in_space = Some(space);
            if wide {
                out.push((std::mem::take(&mut current), span.style.clone()));
                in_space = None;
            }
        }
        if !current.is_empty() {
            out.push((current, span.style.clone()));
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
            Depth::Ansi256 => codes.push(format!("{base};5;{}", theme::to_256(color))),
            Depth::None => {}
        }
    }
    codes.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_from_outside_cannot_put_escapes_on_the_terminal() {
        let line = Line::plain("done\u{1b}]52;c;cm0gLXJmIH4=\u{7} \u{1b}[2Jok");
        assert_eq!(line.text(), "done ok");
        let link = Style {
            link: Some("https://x.dev/\u{1b}\\\u{1b}]0;pwned\u{7}".into()),
            ..Style::default()
        };
        let ansi = Line::styled("x", link).to_ansi(theme::get(), Depth::None);
        assert_eq!(ansi.matches('\u{1b}').count(), 4, "{ansi:?}");
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn widths_count_cells_not_bytes() {
        assert_eq!(width("héllo"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("👍"), 2);
        assert_eq!(Line::plain("a\tb").width(), 6);
    }

    #[test]
    fn wraps_at_words_with_a_hanging_indent() {
        let line = Line::plain("the quick brown fox jumps");
        assert_eq!(
            texts(&line.wrap(10, &Line::plain("  "))),
            vec!["the quick", "  brown", "  fox", "  jumps"]
        );
    }

    #[test]
    fn long_words_and_wide_characters_break_anywhere() {
        let line = Line::plain("abcdefghij");
        assert_eq!(
            texts(&line.wrap(4, &Line::default())),
            vec!["abcd", "efgh", "ij"]
        );
        let cjk = Line::plain("日本語テキスト");
        assert!(cjk.wrap(6, &Line::default()).iter().all(|l| l.width() <= 6));
    }

    #[test]
    fn wrapping_keeps_styles() {
        let mut line = Line::plain("plain ");
        line.push("bold words here", Style::default().bold());
        let wrapped = line.wrap(11, &Line::default());
        assert_eq!(wrapped[0].to_tagged(), "plain [bold]bold[/]");
        assert_eq!(wrapped[1].to_tagged(), "[bold]words here[/]");
    }

    #[test]
    fn truncates_with_an_ellipsis() {
        assert_eq!(Line::plain("hello world").truncate(6).text(), "hello…");
        assert_eq!(Line::plain("hi").truncate(6).text(), "hi");
    }

    #[test]
    fn ansi_resets_after_every_styled_span() {
        let line = Line::styled("ok", Style::fg("accent").bold());
        let ansi = line.to_ansi(theme::get(), Depth::TrueColor);
        assert_eq!(ansi, "\x1b[1;38;2;0;220;130mok\x1b[0m");
        assert_eq!(line.to_ansi(theme::get(), Depth::None), "\x1b[1mok\x1b[0m");
        assert_eq!(
            line.to_ansi(theme::get(), Depth::Ansi256),
            "\x1b[1;38;5;42mok\x1b[0m"
        );
    }
}
