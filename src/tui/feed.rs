//! The conversation as it appears on screen. Agent events arrive here and become blocks, kept as
//! their source so any width can redraw them and thinking can be opened and closed; whatever is
//! still in flight is drawn fresh after them.

use std::time::{Duration, Instant};

use crate::agent::event::{AgentEvent, DeltaKind, Outcome};
use crate::agent::message::{Assistant, Finish, ToolResult};
use crate::api::error::ErrorInfo;
use crate::limits;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::text::{markdown, stream};
use crate::tui::cards::{self, State};
use crate::tui::{ui, welcome};

pub enum Block {
    Welcome(welcome::Info),
    User(String),
    /// The model's prose; `first` starts a stretch of it, after the user or a tool card.
    Reply {
        md: String,
        first: bool,
    },
    /// Anything else in markdown: what a command answered.
    Markdown(String),
    Notice {
        text: String,
        role: String,
    },
    Tool {
        arguments: String,
        result: ToolResult,
    },
    Thinking {
        text: String,
        secs: u64,
    },
    Error(ErrorInfo),
}

struct Entry {
    block: Block,
    /// A thinking block shows its text.
    open: bool,
    /// The block drawn at a width; cleared when it is toggled.
    drawn: Option<(usize, Vec<Line>)>,
}

impl Entry {
    fn lines(&mut self, width: usize) -> &[Line] {
        if self.drawn.as_ref().is_none_or(|(w, _)| *w != width) {
            self.drawn = Some((width, render(&self.block, self.open, width)));
        }
        self.drawn
            .as_ref()
            .map_or(&[], |(_, lines)| lines.as_slice())
    }
}

struct Running {
    call_id: String,
    name: String,
    arguments: String,
    since: Instant,
}

pub struct Feed {
    entries: Vec<Entry>,
    /// From the last `measure`: the row each entry starts on, the live part and where it starts.
    starts: Vec<usize>,
    live: Vec<Line>,
    live_start: usize,
    total: usize,
    /// New thinking blocks start open.
    open_thinking: bool,
    /// The assistant text of the reply in flight, and how much of it is already a block.
    text: String,
    committed: usize,
    reasoning: String,
    reasoning_since: Option<Instant>,
    running: Vec<Running>,
    retry: Option<(Instant, String)>,
}

impl Feed {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            starts: Vec::new(),
            live: Vec::new(),
            live_start: 0,
            total: 0,
            open_thinking: false,
            text: String::new(),
            committed: 0,
            reasoning: String::new(),
            reasoning_since: None,
            running: Vec::new(),
            retry: None,
        }
    }

    pub fn push(&mut self, block: Block) {
        let open = self.open_thinking;
        self.entries.push(Entry {
            block,
            open,
            drawn: None,
        });
    }

    pub fn user(&mut self, text: &str) {
        self.push(Block::User(text.to_owned()));
    }

    pub fn markdown(&mut self, md: &str) {
        self.push(Block::Markdown(md.to_owned()));
    }

    pub fn notice(&mut self, text: &str, role: &str) {
        self.push(Block::Notice {
            text: text.to_owned(),
            role: role.to_owned(),
        });
    }

    /// What copying block `index` puts on the clipboard: a message as it was written, a reply
    /// whole (all the blocks of its stretch), a tool card's output.
    pub fn copy_text(&self, index: usize) -> Option<String> {
        let text = match &self.entries.get(index)?.block {
            Block::Welcome(_) => return None,
            Block::User(text) | Block::Markdown(text) | Block::Notice { text, .. } => text.clone(),
            Block::Thinking { text, .. } => text.clone(),
            Block::Tool { result, .. } => result.content.clone(),
            Block::Error(error) => error.to_string(),
            Block::Reply { .. } => {
                let reply = |i: usize| match &self.entries[i].block {
                    Block::Reply { md, first } => Some((md.as_str(), *first)),
                    _ => None,
                };
                let start = (0..=index)
                    .rev()
                    .find(|&i| reply(i).is_some_and(|(_, first)| first))?;
                (start..self.entries.len())
                    .map_while(|i| reply(i).filter(|&(_, first)| !first || i == start))
                    .map(|(md, _)| md)
                    .collect()
            }
        };
        Some(text.trim_end().to_owned())
    }

    /// Opens or closes the thinking block at `index`; `false` when it is not one.
    pub fn toggle(&mut self, index: usize) -> bool {
        match self.entries.get_mut(index) {
            Some(entry) if matches!(entry.block, Block::Thinking { .. }) => {
                entry.open = !entry.open;
                entry.drawn = None;
                true
            }
            _ => false,
        }
    }

    /// Opens every thinking block, or closes them all when all are open; new ones follow suit.
    pub fn toggle_all(&mut self) {
        let thinking = self
            .entries
            .iter_mut()
            .filter(|e| matches!(e.block, Block::Thinking { .. }));
        let mut all: Vec<&mut Entry> = thinking.collect();
        let open = if all.is_empty() {
            !self.open_thinking
        } else {
            !all.iter().all(|e| e.open)
        };
        for entry in &mut all {
            entry.open = open;
            entry.drawn = None;
        }
        self.open_thinking = open;
    }

    pub fn event(&mut self, event: &AgentEvent, now: Instant) {
        match event {
            AgentEvent::Delta { kind, text, .. } => {
                self.retry = None;
                match kind {
                    DeltaKind::Reasoning => {
                        self.reasoning_since.get_or_insert(now);
                        self.reasoning.push_str(text);
                    }
                    DeltaKind::Text => {
                        self.finish_reasoning(now);
                        self.text.push_str(text);
                        self.commit_complete();
                    }
                    DeltaKind::ToolCall => {}
                }
            }
            AgentEvent::MessageEnd { message } => {
                self.retry = None;
                self.finish_reasoning(now);
                self.finish_text();
                self.outcome_of(message);
            }
            AgentEvent::ToolStart {
                call_id,
                name,
                arguments,
            } => self.running.push(Running {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                since: now,
            }),
            AgentEvent::ToolEnd { result } => {
                let arguments = self
                    .running
                    .iter()
                    .position(|r| r.call_id == result.call_id)
                    .map(|i| self.running.remove(i).arguments)
                    .unwrap_or_default();
                self.push(Block::Tool {
                    arguments,
                    result: result.clone(),
                });
            }
            AgentEvent::Retry {
                delay_ms, error, ..
            } => {
                let until = now + Duration::from_millis(*delay_ms);
                self.retry = Some((until, error.entry().title.clone()));
            }
            AgentEvent::Unsaved { path, error } => {
                let text = fill(&ui::get().text.unsaved, &[("path", path), ("error", error)]);
                self.notice(&text, "warn");
            }
            AgentEvent::RunEnd { outcome } => {
                self.finish_reasoning(now);
                self.finish_text();
                self.running.clear();
                self.retry = None;
                let limit = limits::get().agent.max_turns.value.to_string();
                let note = match outcome {
                    Outcome::Aborted => Some(ui::get().text.interrupted.clone()),
                    Outcome::Rejected => Some(ui::get().text.declined.clone()),
                    Outcome::TurnLimit => {
                        Some(fill(&ui::get().outcome.turn_limit, &[("count", &limit)]))
                    }
                    _ => None,
                };
                if let Some(note) = note {
                    self.notice(&note, "muted");
                }
            }
            AgentEvent::Delivered { text } => self.user(text),
            AgentEvent::Prompt { .. } => {}
        }
    }

    /// Lays the transcript out at `width` and returns its height in rows: blocks one blank
    /// row apart, then the part still in flight.
    pub fn measure(&mut self, width: usize, now: Instant) -> usize {
        let mut row = 0;
        self.starts.clear();
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if i > 0 {
                row += 1;
            }
            self.starts.push(row);
            row += entry.lines(width).len();
        }
        self.live = self.live_lines(width, now);
        if !self.live.is_empty() && !self.entries.is_empty() {
            row += 1;
        }
        self.live_start = row;
        self.total = row + self.live.len();
        self.total
    }

    /// Rows `from..to` of the last `measure`, each with the block it belongs to.
    pub fn rows(&self, from: usize, to: usize) -> Vec<(Line, Option<usize>)> {
        (from..to.min(self.total))
            .map(|row| {
                if row >= self.live_start {
                    return (self.live[row - self.live_start].clone(), None);
                }
                let i = self.starts.partition_point(|&s| s <= row) - 1;
                let lines = self.entries[i]
                    .drawn
                    .as_ref()
                    .map_or(&[][..], |(_, l)| l.as_slice());
                match lines.get(row - self.starts[i]) {
                    Some(line) => (line.clone(), Some(i)),
                    None => (Line::default(), None),
                }
            })
            .collect()
    }

    /// Seconds left before a retry, and why it is needed.
    pub fn retry(&self, now: Instant) -> Option<(u64, &str)> {
        self.retry.as_ref().map(|(until, why)| {
            (
                until.saturating_duration_since(now).as_secs() + 1,
                why.as_str(),
            )
        })
    }

    /// The part still in flight: thinking, the reply's unfinished tail, running tools.
    fn live_lines(&self, width: usize, now: Instant) -> Vec<Line> {
        let mut groups: Vec<Vec<Line>> = Vec::new();
        if !self.reasoning.is_empty() {
            let secs = self
                .reasoning_since
                .map_or(0, |since| now.duration_since(since).as_secs());
            let text = &ui::get().thinking;
            let header = fill(&text.live, &[("secs", &secs.to_string())]);
            let mut lines = thinking_lines(&text.open, &header, &self.reasoning, width);
            if !self.open_thinking {
                let tail = limits::get().tui.reasoning_tail_lines.value;
                let skip = (lines.len() - 1).saturating_sub(tail);
                lines.drain(1..1 + skip);
            }
            groups.push(lines);
        }
        let tail = stream::trim_partial_fence(&self.text[self.committed..]);
        if !tail.trim().is_empty() {
            groups.push(reply_lines(tail, self.committed == 0, width));
        }
        let limits = &limits::get().tui;
        if !self.running.is_empty() {
            groups.push(
                self.running
                    .iter()
                    .map(|tool| {
                        let elapsed = now.duration_since(tool.since).as_millis();
                        let frame = ui::get().spinner_frame(elapsed, limits.spinner_ms.value);
                        cards::header(&tool.name, &tool.arguments, State::Running(frame))
                            .truncate(width)
                    })
                    .collect(),
            );
        }
        let mut lines = Vec::new();
        for (i, group) in groups.into_iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.extend(group);
        }
        lines
    }

    fn commit_complete(&mut self) {
        let pending = &self.text[self.committed..];
        let cut = stream::complete_prefix(pending);
        if cut > 0 {
            let md = pending[..cut].to_owned();
            let first = self.committed == 0;
            self.committed += cut;
            self.push(Block::Reply { md, first });
        }
    }

    fn finish_text(&mut self) {
        let rest = self.text[self.committed..].to_owned();
        if !rest.trim().is_empty() {
            let first = self.committed == 0;
            self.push(Block::Reply { md: rest, first });
        }
        self.text.clear();
        self.committed = 0;
    }

    fn finish_reasoning(&mut self, now: Instant) {
        if self.reasoning.is_empty() {
            return;
        }
        let secs = self
            .reasoning_since
            .map_or(0, |since| now.duration_since(since).as_secs());
        let text = std::mem::take(&mut self.reasoning);
        self.reasoning_since = None;
        self.push(Block::Thinking { text, secs });
    }

    fn outcome_of(&mut self, message: &Assistant) {
        if let Some(error) = &message.error {
            self.push(Block::Error(error.clone()));
        } else if message.finish == Finish::Length {
            let text = &ui::get().text.cut_off;
            self.notice(text, "warn");
        }
    }
}

fn render(block: &Block, open: bool, width: usize) -> Vec<Line> {
    match block {
        Block::Welcome(info) => welcome::render(info, width),
        Block::User(text) => user_lines(text, width),
        Block::Reply { md, first } => reply_lines(md, *first, width),
        Block::Markdown(md) => markdown::render(md, width),
        Block::Notice { text, role } => {
            Line::styled(text, Style::fg(role)).wrap(width, &Line::default())
        }
        Block::Tool { arguments, result } => {
            cards::finished(&result.name, arguments, result, width)
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
fn thinking_lines(mark: &str, header: &str, text: &str, width: usize) -> Vec<Line> {
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

/// The user's message on a full-width band, a blank band row above and below it.
fn user_lines(text: &str, width: usize) -> Vec<Line> {
    let band = Style::fg("user_text").on("user_band");
    let prompt = Line::styled(&ui::get().prompt, Style::fg("user").bold().on("user_band"));
    let indent = Line::styled(" ".repeat(prompt.width()), band.clone());
    let mut lines = vec![Line::default()];
    for (i, raw) in text.lines().enumerate() {
        let mut line = if i == 0 {
            prompt.clone()
        } else {
            indent.clone()
        };
        line.push(raw, band.clone());
        lines.extend(line.wrap(width, &indent));
    }
    lines.push(Line::default());
    lines.into_iter().map(|l| l.padded(width, &band)).collect()
}

/// The model's prose under a hanging indent, marked where a stretch of it starts.
fn reply_lines(md: &str, first: bool, width: usize) -> Vec<Line> {
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
    let entry = error.entry();
    let mut lines = vec![Line::styled(
        format!("● {}", entry.title),
        Style::fg("error").bold(),
    )];
    let indent = Line::plain("  ");
    let mut detail = indent.clone();
    detail.push(&error.message, Style::default().dim());
    lines.extend(detail.wrap(width, &indent));
    if let Some(hint) = &entry.hint {
        let mut line = indent.clone();
        line.push(hint, Style::fg("muted"));
        lines.extend(line.wrap(width, &indent));
    }
    if let Some(id) = &error.request_id {
        let mut line = indent.clone();
        line.push(format!("request {id}"), Style::fg("muted"));
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Assistant, ToolResult};
    use crate::api::error::ErrorInfo;

    fn delta(kind: DeltaKind, text: &str) -> AgentEvent {
        AgentEvent::Delta {
            kind,
            text: text.into(),
            index: None,
            name: None,
        }
    }

    fn end(error: Option<ErrorInfo>, finish: Finish) -> AgentEvent {
        AgentEvent::MessageEnd {
            message: Assistant {
                parts: Vec::new(),
                model: "m".into(),
                effort: None,
                usage: None,
                finish,
                error,
                request_id: None,
            },
        }
    }

    fn shown(feed: &mut Feed, width: usize, now: Instant) -> Vec<(String, Option<usize>)> {
        let total = feed.measure(width, now);
        feed.rows(0, total)
            .into_iter()
            .map(|(line, block)| (line.text(), block))
            .collect()
    }

    /// Without the padding that carries a band's background to the edge.
    fn texts(feed: &mut Feed, now: Instant) -> Vec<String> {
        shown(feed, 60, now)
            .into_iter()
            .map(|(t, _)| t.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn finished_paragraphs_become_blocks_while_the_tail_stays_live() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.event(&delta(DeltaKind::Text, "First para"), now);
        assert_eq!(shown(&mut feed, 60, now), [("◆ First para".into(), None)]);
        // Committed paragraphs keep one mark for the stretch; the rest hang under it.
        feed.event(&delta(DeltaKind::Text, "graph.\n\nSecond"), now);
        assert_eq!(
            shown(&mut feed, 60, now),
            [
                ("◆ First paragraph.".into(), Some(0)),
                (String::new(), None),
                ("  Second".into(), None)
            ]
        );
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(
            shown(&mut feed, 60, now),
            [
                ("◆ First paragraph.".into(), Some(0)),
                (String::new(), None),
                ("  Second".into(), Some(1))
            ]
        );
    }

    #[test]
    fn thinking_streams_live_then_collapses_and_opens_on_toggle() {
        let t0 = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.event(&delta(DeltaKind::Reasoning, "let me think"), t0);
        assert_eq!(texts(&mut feed, t0), ["▾ Thinking… 0s", "│ let me think"]);
        let t4 = t0 + Duration::from_secs(4);
        feed.event(&delta(DeltaKind::Text, "Answer"), t4);
        assert_eq!(texts(&mut feed, t4), ["▸ Thought for 4s", "", "◆ Answer"]);
        assert!(feed.toggle(0));
        assert_eq!(
            texts(&mut feed, t4)[..2],
            ["▾ Thought for 4s", "│ let me think"]
        );
        assert!(!feed.toggle(5));
    }

    #[test]
    fn toggling_all_opens_every_block_and_the_next_ones() {
        let t0 = crate::io::clock::instant();
        let mut feed = Feed::new();
        for word in ["one", "two"] {
            feed.event(&delta(DeltaKind::Reasoning, word), t0);
            feed.event(&end(None, Finish::Done), t0);
        }
        feed.toggle(0);
        feed.toggle_all();
        let rows = texts(&mut feed, t0);
        assert!(rows.contains(&"│ one".to_owned()) && rows.contains(&"│ two".to_owned()));
        feed.event(&delta(DeltaKind::Reasoning, "three"), t0);
        feed.event(&end(None, Finish::Done), t0);
        assert!(texts(&mut feed, t0).contains(&"│ three".to_owned()));
        feed.toggle_all();
        assert!(!texts(&mut feed, t0).iter().any(|r| r.starts_with('│')));
    }

    #[test]
    fn tools_run_live_and_become_cards() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.event(
            &AgentEvent::ToolStart {
                call_id: "c1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            now,
        );
        assert!(texts(&mut feed, now)[0].ends_with("bash ls"));
        feed.event(
            &AgentEvent::ToolEnd {
                result: ToolResult {
                    call_id: "c1".into(),
                    name: "bash".into(),
                    content: "a.txt".into(),
                    is_error: false,
                    details: None,
                },
            },
            now,
        );
        assert_eq!(texts(&mut feed, now), ["● bash ls", "  a.txt"]);
    }

    #[test]
    fn errors_become_cards_with_hint_and_request_id() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        let error = ErrorInfo {
            request_id: Some("req_9".into()),
            ..ErrorInfo::local("insufficient_balance", "top up")
        };
        feed.event(&end(Some(error), Finish::Error), now);
        let lines = texts(&mut feed, now);
        assert_eq!(lines[0], "● Out of funds");
        assert!(lines.iter().any(|l| l.contains("req_9")));
    }

    #[test]
    fn blocks_are_one_blank_row_apart_and_rewrap_with_the_width() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.user("hi");
        feed.event(&delta(DeltaKind::Text, "hello"), now);
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(texts(&mut feed, now), ["", "› hi", "", "", "◆ hello"]);
        feed.markdown(&"word ".repeat(20));
        let wide = feed.measure(60, now);
        let narrow = feed.measure(20, now);
        assert!(narrow > wide, "{narrow} vs {wide}");
        assert_eq!(feed.rows(1, 3).len(), 2);
    }

    #[test]
    fn copying_takes_a_block_whole_and_a_reply_with_all_its_paragraphs() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.user("fix it");
        feed.event(&delta(DeltaKind::Text, "First.\n\nSecond."), now);
        feed.event(&end(None, Finish::ToolCalls), now);
        feed.event(&delta(DeltaKind::Text, "Done."), now);
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(feed.copy_text(0).as_deref(), Some("fix it"));
        // The second paragraph was its own block; either one copies the reply.
        assert_eq!(feed.copy_text(1).as_deref(), Some("First.\n\nSecond."));
        assert_eq!(feed.copy_text(2).as_deref(), Some("First.\n\nSecond."));
        assert_eq!(feed.copy_text(3).as_deref(), Some("Done."));
        assert_eq!(feed.copy_text(9), None);
    }

    #[test]
    fn the_users_messages_sit_on_a_band_and_each_stretch_of_prose_is_marked() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.user("first line\nsecond line");
        let band: Vec<Line> = {
            feed.measure(40, now);
            feed.rows(0, 4).into_iter().map(|(l, _)| l).collect()
        };
        for line in &band {
            assert_eq!(line.width(), 40, "{:?}", line.text());
            assert!(
                line.spans
                    .iter()
                    .all(|s| s.style.bg.as_deref() == Some("user_band"))
            );
        }
        assert_eq!(band[2].text().trim_end(), "  second line");

        feed.event(&delta(DeltaKind::Text, "Looking."), now);
        feed.event(&end(None, Finish::ToolCalls), now);
        feed.event(&delta(DeltaKind::Text, "Found it."), now);
        feed.event(&end(None, Finish::Done), now);
        feed.markdown("Commands: help");
        let rows = texts(&mut feed, now);
        assert_eq!(
            rows[rows.len() - 5..],
            ["◆ Looking.", "", "◆ Found it.", "", "Commands: help"]
        );
    }
}
