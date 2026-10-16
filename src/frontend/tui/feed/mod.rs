//! The conversation as it appears on screen. Agent events arrive here and become blocks, kept as
//! their source so any width can redraw them and thinking can be opened and closed; whatever is
//! still in flight is drawn fresh after them.

mod render;
mod replay;

use std::time::Instant;

use crate::config::limits;
use crate::conversation::error::ErrorInfo;
use crate::conversation::message::{Assistant, Finish, ToolResult};
use crate::frontend::tui::calls::{self, State};
use crate::frontend::tui::{progress, ui, welcome};
use crate::frontend::words;
use crate::live::event::{AgentEvent, DeltaKind, Ended, Outcome};
use crate::text::stream;
use crate::text::styled::Line;
use crate::text::template::fill;
use render::{render, reply_lines, thinking_lines};

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
        /// How long it took; `None` for thinking read back from a session file.
        secs: Option<u64>,
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
    /// A subagent's latest call, by tool and arguments, for the `task` call that started it.
    step: Option<(String, String)>,
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
    /// Why the summary in progress was made (`auto`, `overflow`), for its end to say.
    compacting: (bool, bool),
    /// The model's context window, to tell a reply cut off by it from one out of output.
    window: u64,
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
            compacting: (false, false),
            window: 0,
        }
    }

    /// The window of the model the session uses from now on.
    pub fn set_window(&mut self, window: u64) {
        self.window = window;
    }

    pub fn push(&mut self, block: Block) {
        let open = self.open_thinking;
        self.entries.push(Entry {
            block,
            open,
            drawn: None,
        });
    }

    /// The welcome, in place of the one already shown, so it follows the account.
    pub fn set_welcome(&mut self, info: welcome::Info) {
        let shown = self
            .entries
            .iter_mut()
            .find(|e| matches!(e.block, Block::Welcome(_)));
        match shown {
            Some(entry) => {
                entry.block = Block::Welcome(info);
                entry.drawn = None;
            }
            None => self.push(Block::Welcome(info)),
        }
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

    /// The model's last reply, whole, for copying.
    pub fn last_reply(&self) -> Option<String> {
        let start = self
            .entries
            .iter()
            .rposition(|e| matches!(e.block, Block::Reply { first: true, .. }))?;
        self.copy_text(start)
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
                match kind {
                    DeltaKind::Reasoning => {
                        self.reasoning_since.get_or_insert(now);
                        self.reasoning.push_str(text);
                    }
                    DeltaKind::Text => {
                        self.finish_reasoning(now);
                        self.text.push_str(text);
                        // A reply's leading blank lines would commit as an empty, marked block.
                        if self.committed == 0 {
                            self.text.drain(..stream::blank_lead(&self.text));
                        }
                        self.commit_complete();
                    }
                    DeltaKind::ToolCall => {}
                }
            }
            AgentEvent::MessageEnd { message } => {
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
                step: None,
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
            AgentEvent::Unsaved { path, error } => {
                let text = fill(&words::get().unsaved, &[("path", path), ("error", error)]);
                self.notice(&text, "warn");
            }
            AgentEvent::RunEnd(Ended { outcome, turns }) => {
                self.finish_reasoning(now);
                self.finish_text();
                self.running.clear();
                let note = match outcome {
                    Outcome::Aborted => Some(words::get().interrupted.clone()),
                    Outcome::Rejected => Some(ui::get().text.declined.clone()),
                    Outcome::TurnLimit => {
                        let count = turns.to_string();
                        let stopped = fill(&words::get().turn_limit, &[("count", &count)]);
                        Some(fill(
                            &ui::get().outcome.turn_limit,
                            &[("stopped", &stopped)],
                        ))
                    }
                    _ => None,
                };
                if let Some(note) = note {
                    self.notice(&note, "muted");
                }
            }
            AgentEvent::Delivered { text } => self.user(text),
            // The working line shows a retry; a prompt is a card; the session words a stopped
            // snapshot with what /undo can no longer do.
            AgentEvent::CompactionStart { auto, overflow } => self.compacting = (*auto, *overflow),
            AgentEvent::CompactionEnd { .. } => {
                if let Some((text, role)) = words::compaction(event, self.compacting) {
                    self.notice(&text, role);
                }
            }
            // A subagent's work shows as the latest call under the task that started it.
            AgentEvent::Child { call_id, event } => {
                if let AgentEvent::ToolStart {
                    name, arguments, ..
                } = event.innermost()
                    && let Some(task) = self.running.iter_mut().find(|r| r.call_id == *call_id)
                {
                    task.step = Some((name.clone(), arguments.clone()));
                }
            }
            AgentEvent::Retry { .. }
            | AgentEvent::Prompt { .. }
            | AgentEvent::SnapshotOff { .. } => {}
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

    /// The part still in flight: thinking, the reply's unfinished tail, running tools.
    fn live_lines(&self, width: usize, now: Instant) -> Vec<Line> {
        let mut groups: Vec<Vec<Line>> = Vec::new();
        if !self.reasoning.is_empty() {
            let secs = self.reasoning_secs(now);
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
                    .flat_map(|tool| {
                        let elapsed = now.duration_since(tool.since).as_millis();
                        let frame =
                            progress::frame(&ui::get().spinner, elapsed, limits.spinner_ms.value);
                        let mut lines = vec![
                            calls::header(&tool.name, &tool.arguments, State::Running(frame))
                                .truncate(width),
                        ];
                        if let Some((name, arguments)) = &tool.step {
                            lines.push(calls::step(name, arguments).truncate(width));
                        }
                        lines
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
        let secs = self.reasoning_secs(now);
        let text = std::mem::take(&mut self.reasoning);
        self.reasoning_since = None;
        self.push(Block::Thinking {
            text,
            secs: Some(secs),
        });
    }

    /// How long the reasoning in flight has run.
    fn reasoning_secs(&self, now: Instant) -> u64 {
        self.reasoning_since
            .map_or(0, |since| now.duration_since(since).as_secs())
    }

    /// What a finished reply came to, beyond its text: its error, or that it was cut off.
    fn outcome_of(&mut self, message: &Assistant) {
        if let Some(error) = &message.error {
            self.push(Block::Error(error.clone()));
        } else if message.finish == Finish::Length {
            let text = words::cut_off(message, self.window);
            self.notice(text, "warn");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::conversation::error::ErrorInfo;
    use crate::conversation::message::{Assistant, ToolResult};

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
                error,
                ..Assistant::stub(finish, Vec::new())
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
    fn a_reply_streamed_a_character_at_a_time_draws_as_it_does_whole() {
        let now = crate::io::clock::instant();
        let drawn = |md: &str, step: usize| {
            let mut feed = Feed::new();
            let chars: Vec<char> = md.chars().collect();
            for chunk in chars.chunks(step) {
                let text: String = chunk.iter().collect();
                feed.event(&delta(DeltaKind::Text, &text), now);
            }
            feed.event(&end(None, Finish::Done), now);
            texts(&mut feed, now)
        };
        let table = "Here:\n\n| Object | Test |\n|---|---|\n| X | 17 |\n| Y | 18 |\n\nDone.";
        assert!(drawn(table, table.len()).contains(&"  │ Y      │ 18   │".to_owned()));
        for md in [
            table,
            "Steps:\n1. one\n2. two\n\n- a\n- b\n  - nested\n\nAfter.",
            "Run:\n```sh\nls\n\nls -a\n```\nThen:\n\n> quoted\nlazy\n\n# Head\nText",
        ] {
            assert_eq!(drawn(md, 1), drawn(md, md.len()), "{md:?}");
        }
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
    fn a_reply_opening_with_blank_lines_sits_one_row_under_its_thinking() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        feed.event(&delta(DeltaKind::Reasoning, "let me think"), now);
        for chunk in ["\n", " \n\n", "Answer.", "\n\nMore."] {
            feed.event(&delta(DeltaKind::Text, chunk), now);
        }
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(
            texts(&mut feed, now),
            ["▸ Thought for 0s", "", "◆ Answer.", "", "  More."]
        );
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
                    asks: Vec::new(),
                    refused: None,
                    usage: None,
                },
            },
            now,
        );
        assert_eq!(texts(&mut feed, now), ["● bash ls", "  a.txt"]);
    }

    #[test]
    fn a_task_shows_its_subagents_latest_call_then_sums_it_up() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        let start = |call_id: &str, name: &str, arguments: &str| AgentEvent::ToolStart {
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        };
        feed.event(
            &start("c1", "task", r#"{"description":"find the parser"}"#),
            now,
        );
        // The subagent's call ids are its own: `c1` again here.
        let child = AgentEvent::Child {
            call_id: "c1".into(),
            event: Box::new(start("c1", "read", r#"{"path":"src/parse.rs"}"#)),
        };
        feed.event(&child, now);
        let live = texts(&mut feed, now);
        assert!(live[0].ends_with("task find the parser"), "{live:?}");
        assert_eq!(live[1], "  └ read src/parse.rs");
        let result = ToolResult {
            call_id: "c1".into(),
            name: "task".into(),
            content: "it is in src/parse.rs".into(),
            is_error: false,
            details: Some(serde_json::json!({"agent": "explore", "calls": 2})),
            asks: Vec::new(),
            refused: None,
            usage: None,
        };
        feed.event(&AgentEvent::ToolEnd { result }, now);
        assert_eq!(
            texts(&mut feed, now),
            ["● task find the parser", "  explore: 2 calls"]
        );
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
        assert_eq!(texts(&mut feed, now), ["▌", "▌ hi", "▌", "", "◆ hello"]);
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
        assert_eq!(feed.last_reply().as_deref(), Some("Done."));
        assert_eq!(Feed::new().last_reply(), None);
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
        assert_eq!(band[2].text().trim_end(), "▌ second line");

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
