//! How each kind of block looks: the user's band, the reply under its mark, thinking, cards and
//! errors.

use super::Block;
use crate::api::error::Entry as ErrorEntry;
use crate::conversation::error::ErrorInfo;
use crate::frontend::tui::{calls, ui, welcome};
use crate::text::markdown;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

pub fn render(block: &Block, open: bool, width: usize) -> Vec<Line> {
    match block {
        Block::Welcome(info) => welcome::render(info, width),
        Block::User(text) => user_lines(text, width),
        Block::Reply { md, first } => reply_lines(md, *first, width),
        Block::Markdown(md) => markdown::render(md, width),
        Block::Notice { text, role } => {
            Line::styled(text, Style::fg(role)).wrap(width, &Line::default())
        }
        Block::Tool { arguments, result } => {
            calls::finished(&result.name, arguments, result, width)
        }
        Block::Thinking { text, secs } => {
            let words = &ui::get().thinking;
            let header = fill(&words.done, &[("secs", &secs.to_string())]);
            let mark = if open { &words.open } else { &words.closed };
            let mut lines = thinking_lines(mark, &header, text, width);
            if !open {
                lines.truncate(1);
            }
            lines
        }
        Block::Error(error) => error_lines(error, width),
    }
}

/// A thinking header, then the text under a gutter.
pub fn thinking_lines(mark: &str, header: &str, text: &str, width: usize) -> Vec<Line> {
    let words = &ui::get().thinking;
    let muted = Style::fg("muted");
    let mut lines = vec![Line::styled(format!("{mark} {header}"), muted.clone()).truncate(width)];
    let gutter = Line::styled(&words.gutter, muted);
    let style = Style::fg("reasoning").italic();
    for raw in text.trim().lines() {
        let mut line = gutter.clone();
        line.push(raw, style.clone());
        lines.extend(line.wrap(width, &gutter));
    }
    lines
}

/// The user's message on a full-width band, a blank band row above and below it, and the mark
/// down its left edge.
fn user_lines(text: &str, width: usize) -> Vec<Line> {
    let band = Style::fg("user_text").on("user_band");
    let mark = Line::styled(&ui::get().user_mark, Style::fg("user").on("user_band"));
    let mut lines = vec![mark.clone()];
    for raw in text.lines() {
        let mut line = mark.clone();
        line.push(raw, band.clone());
        lines.extend(line.wrap(width, &mark));
    }
    lines.push(mark);
    lines.into_iter().map(|l| l.padded(width, &band)).collect()
}

/// The model's prose under a hanging indent, marked where a stretch of it starts.
pub fn reply_lines(md: &str, first: bool, width: usize) -> Vec<Line> {
    let mark = Line::styled(&ui::get().reply_mark, Style::fg("reply").bold());
    let indent = Line::plain(" ".repeat(mark.width()));
    let mut unmarked = first;
    markdown::render(md, width.saturating_sub(indent.width()))
        .into_iter()
        .map(|line| {
            if line.spans.is_empty() {
                return line;
            }
            let mut out = if std::mem::take(&mut unmarked) {
                mark.clone()
            } else {
                indent.clone()
            };
            out.extend(line);
            out
        })
        .collect()
}

fn error_lines(error: &ErrorInfo, width: usize) -> Vec<Line> {
    let entry = ErrorEntry::of(error);
    let mut lines = vec![Line::styled(
        format!("{} {}", ui::get().marks.done, entry.title),
        Style::fg("error").bold(),
    )];
    let indent = Line::plain("  ");
    let mut detail = indent.clone();
    detail.push(&error.message, Style::default().dim());
    lines.extend(detail.wrap(width, &indent));
    if let Some(hint) = entry.hint() {
        let mut line = indent.clone();
        line.push(hint, Style::fg("muted"));
        lines.extend(line.wrap(width, &indent));
    }
    if let Some(id) = &error.request_id {
        let mut line = indent.clone();
        line.push(
            fill(&ui::get().text.request, &[("id", id)]),
            Style::fg("muted"),
        );
        lines.push(line);
    }
    lines
}
