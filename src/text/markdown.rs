//! Markdown as styled lines at a given width.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::text::highlight;
use crate::text::styled::{Line, Style, width};
use crate::text::table::{self, Align};

/// Longest a horizontal rule gets, so it reads as a divider rather than a wall.
const RULE_MAX: usize = 80;

/// The markdown crowbot draws; the stream cut parses with the same, so both see one structure.
pub fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

pub fn render(md: &str, width: usize) -> Vec<Line> {
    let mut r = Renderer {
        // Narrower than this, prefixes and borders leave no room for a word.
        width: width.max(8),
        ..Renderer::default()
    };
    for event in Parser::new_ext(md, options()) {
        r.event(event);
    }
    r.flush();
    r.out
}

#[derive(Default)]
struct Renderer {
    width: usize,
    out: Vec<Line>,
    /// Inline content of the block being built.
    inline: Line,
    styles: Vec<Style>,
    containers: Vec<Container>,
    /// A block ended; the next one starts after a blank line.
    gap: bool,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
    link: Option<String>,
    table: Option<Table>,
}

enum Container {
    Quote,
    List { next: Option<u64> },
    Item { marker: String, used: bool },
}

#[derive(Default)]
struct Table {
    aligns: Vec<Align>,
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
}

impl Renderer {
    fn event(&mut self, event: Event<'_>) {
        // A table keeps only its structure here; cell content takes the inline path below.
        if let Some(table) = &mut self.table {
            match event {
                Event::Start(Tag::TableHead) => {
                    self.styles.push(self.current().bold());
                    return;
                }
                Event::Start(Tag::TableRow | Tag::TableCell) => return,
                Event::End(TagEnd::TableCell) => {
                    table.row.push(std::mem::take(&mut self.inline));
                    return;
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                    if matches!(event, Event::End(TagEnd::TableHead)) {
                        self.styles.pop();
                    }
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                    return;
                }
                Event::End(TagEnd::Table) => {
                    let table = self.table.take().unwrap_or_default();
                    self.block_start();
                    self.table_lines(table);
                    self.gap = true;
                    return;
                }
                _ => {}
            }
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&text);
                } else {
                    let style = self.current();
                    self.inline.push(text.to_string(), style);
                }
            }
            Event::Code(text) => {
                let style = self.current().patch(&Style::fg("code"));
                self.inline.push(text.to_string(), style);
            }
            Event::SoftBreak => self.inline.push(" ", self.current()),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.block_start();
                let rule = "─".repeat(self.width.min(RULE_MAX));
                self.emit(vec![Line::styled(rule, Style::fg("rule"))]);
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.inline.push(mark, Style::fg("muted"));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.inline
                    .push(html.trim_end().to_string(), Style::fg("muted"));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.block_start(),
            Tag::Heading { level, .. } => {
                self.block_start();
                self.heading = Some(level);
                let style = match level {
                    HeadingLevel::H1 | HeadingLevel::H2 => Style::fg("heading").bold(),
                    _ => Style::default().bold(),
                };
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.block_start();
                self.containers.push(Container::Quote);
                self.styles
                    .push(self.current().patch(&Style::fg("quote").italic()));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.block_start();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(first) => {
                self.flush();
                if !matches!(self.containers.last(), Some(Container::Item { .. })) {
                    self.block_start();
                }
                self.containers.push(Container::List { next: first });
            }
            Tag::Item => {
                self.flush();
                let marker = match self.containers.last_mut() {
                    Some(Container::List { next: Some(n) }) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_owned(),
                };
                self.containers.push(Container::Item {
                    marker,
                    used: false,
                });
            }
            Tag::Emphasis => self.styles.push(self.current().italic()),
            Tag::Strong => self
                .styles
                .push(self.current().patch(&Style::fg("strong").bold())),
            Tag::Strikethrough => {
                let mut style = self.current();
                style.strike = true;
                self.styles.push(style);
            }
            Tag::Link { dest_url, .. } => {
                let mut style = self.current().patch(&Style::fg("link"));
                style.underline = true;
                style.link = Some(dest_url.to_string());
                self.link = Some(dest_url.to_string());
                self.styles.push(style);
            }
            Tag::Image { dest_url, .. } => {
                self.inline
                    .push(format!("[image: {dest_url}]"), Style::fg("muted"));
                self.styles.push(Style::fg("muted"));
            }
            Tag::Table(aligns) => {
                self.flush();
                self.table = Some(Table {
                    aligns: aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Right => Align::Right,
                            _ => Align::Left,
                        })
                        .collect(),
                    ..Table::default()
                });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.heading = None;
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.code_lines(&lang, &code);
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.containers.pop();
                self.gap = true;
            }
            TagEnd::Item => {
                self.flush();
                self.containers.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                // In a table cell the link itself carries the URL; a suffix would crowd it.
                if let Some(url) = self.link.take().filter(|_| self.table.is_none()) {
                    let shown = self.inline.text();
                    if !shown.ends_with(&url) {
                        self.inline.push(format!(" ({url})"), Style::fg("muted"));
                    }
                }
            }
            _ => {}
        }
    }

    fn current(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    /// Starts a block, leaving a blank line after the previous one.
    fn block_start(&mut self) {
        if self.gap && !self.out.is_empty() {
            let (first, _) = self.prefixes(false);
            self.out.push(first);
        }
        self.gap = false;
    }

    /// Emits the pending inline text, wrapped inside the current containers.
    fn flush(&mut self) {
        if self.inline.spans.is_empty() {
            return;
        }
        let inline = std::mem::take(&mut self.inline);
        let (first, rest) = self.prefixes(true);
        let avail = self.width.saturating_sub(first.width()).max(1);
        let wrapped = inline.wrap(avail, &Line::default());
        let mut lines = Vec::with_capacity(wrapped.len());
        for (i, line) in wrapped.into_iter().enumerate() {
            let mut out = if i == 0 { first.clone() } else { rest.clone() };
            out.extend(line);
            lines.push(out);
        }
        self.out.extend(lines);
    }

    fn emit(&mut self, lines: Vec<Line>) {
        let (first, rest) = self.prefixes(true);
        for (i, line) in lines.into_iter().enumerate() {
            let mut out = if i == 0 { first.clone() } else { rest.clone() };
            out.extend(line);
            self.out.push(out);
        }
    }

    /// Prefixes for the first and later lines of a block: quote bars and list markers.
    /// `claim` uses up a list item's marker, so only its first line shows it.
    fn prefixes(&mut self, claim: bool) -> (Line, Line) {
        let mut first = Line::default();
        let mut rest = Line::default();
        for c in &mut self.containers {
            match c {
                Container::Quote => {
                    first.push("│ ", Style::fg("quote"));
                    rest.push("│ ", Style::fg("quote"));
                }
                Container::List { .. } => {}
                Container::Item { marker, used } => {
                    let pad = " ".repeat(width(marker));
                    if *used || !claim {
                        first.push(pad.clone(), Style::default());
                    } else {
                        first.push(marker.clone(), Style::fg("bullet"));
                        *used = true;
                    }
                    rest.push(pad, Style::default());
                }
            }
        }
        (first, rest)
    }

    fn code_lines(&mut self, lang: &str, code: &str) {
        let fence = Style::fg("muted");
        let mut lines = vec![Line::styled(format!("```{lang}"), fence.clone())];
        let (first, _) = self.prefixes(false);
        let avail = self.width.saturating_sub(first.width() + 2).max(1);
        for line in highlight::highlight(code.trim_end_matches('\n'), lang) {
            for part in line.wrap(avail, &Line::default()) {
                let mut indented = Line::plain("  ");
                indented.extend(part);
                lines.push(indented);
            }
        }
        lines.push(Line::styled("```", fence));
        self.emit(lines);
    }

    fn table_lines(&mut self, table: Table) {
        let (first, _) = self.prefixes(false);
        let room = self.width.saturating_sub(first.width());
        self.emit(table::grid(&table.rows, &table.aligns, room));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged(md: &str, width: usize) -> String {
        render(md, width)
            .iter()
            .map(Line::to_tagged)
            .collect::<Vec<_>>()
            .join("\n")
    }

    const SAMPLE: &str = "# Title\n\nSome *emphasis*, **strong** and `code` with a [link](https://crowbot.sh).\n\n- one\n- two\n  - nested item that is long enough to wrap around\n\n1. first\n2. second\n\n> quoted text\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 22 |\n\n---\n\n- [x] done\n- [ ] todo\n";

    #[test]
    fn table_cells_keep_their_inline_styles() {
        let md = "| a | b |\n|---|---|\n| `x` | [site](https://s.dev) ~~old~~ |\n";
        let out = tagged(md, 60);
        assert!(out.contains("[bold]a[/]"), "{out}");
        assert!(out.contains("[code]x[/]"), "{out}");
        assert!(out.contains("link]site[/]"), "{out}");
        assert!(out.contains("strike]old[/]"), "{out}");
        assert!(!out.contains("(https://s.dev)"), "{out}");
    }

    #[test]
    fn renders_every_element() {
        insta::assert_snapshot!("markdown_80", tagged(SAMPLE, 80));
        insta::assert_snapshot!("markdown_40", tagged(SAMPLE, 40));
    }

    #[test]
    fn nothing_is_wider_than_asked() {
        let head: String = (1..=12).map(|i| format!("| column {i} ")).collect();
        let wide = format!("{head}|\n{}|\n", "|---".repeat(12));
        for md in [SAMPLE, wide.as_str()] {
            for w in [20, 40, 80, 120] {
                for line in render(md, w) {
                    assert!(line.width() <= w, "{} > {w}: {}", line.width(), line.text());
                }
            }
        }
    }
}
