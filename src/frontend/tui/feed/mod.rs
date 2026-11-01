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
        /// The first call of its reply; a batch's later calls (`false`) stack with no blank above,
        /// so several calls made in one reply read as one group.
        first: bool,
        /// Nesting under a subagent: 0 top-level, ≥1 a `task`'s descendants, indented by rails.
        depth: usize,
    },
    Thinking {
        text: String,
        /// How long it took; `None` for thinking read back from a session file.
        secs: Option<u64>,
    },
    Error(ErrorInfo),
}

/// The left-gutter a tool card draws behind: `depth` subagent-tree rails, plus one batch rail when
/// the call is one of a reply's grouped calls. Part of the draw cache key, so a card re-renders
/// when a sibling joins its batch.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Decor {
    depth: usize,
    rail: bool,
}

impl Decor {
    /// How many columns of gutter precede the card.
    fn cols(self) -> usize {
        self.depth + usize::from(self.rail)
    }
}

struct Entry {
    block: Block,
    /// A thinking block shows its text; a `task` shows its subagent's child cards.
    open: bool,
    /// The block drawn at a width and gutter; cleared when either changes.
    drawn: Option<(usize, Decor, Vec<Line>)>,
}

impl Entry {
    fn lines(&mut self, width: usize, decor: Decor) -> &[Line] {
        if self
            .drawn
            .as_ref()
            .is_none_or(|(w, d, _)| *w != width || *d != decor)
        {
            self.drawn = Some((width, decor, render(&self.block, self.open, width, decor)));
        }
        self.drawn
            .as_ref()
            .map_or(&[], |(_, _, lines)| lines.as_slice())
    }

    fn depth(&self) -> usize {
        match &self.block {
            Block::Tool { depth, .. } => *depth,
            _ => 0,
        }
    }
}

struct Running {
    call_id: String,
    name: String,
    arguments: String,
    since: Instant,
    /// The subagent's calls so far (a tree — a subagent's own subagents nest), for the `task` call
    /// that started it: shown live, then flattened into the transcript when the task ends.
    children: Vec<SubCall>,
}

/// One call a subagent made, captured from its `Child` events.
struct SubCall {
    call_id: String,
    name: String,
    arguments: String,
    result: Option<ToolResult>,
    children: Vec<SubCall>,
}

/// Routes a subagent's event into the captured tree: a `Child` descends one call-id level (so a
/// subagent's own subagents nest), a `ToolStart` adds a call, a `ToolEnd` fills its result.
fn route(nodes: &mut Vec<SubCall>, event: &AgentEvent) {
    match event {
        AgentEvent::Child { call_id, event } => {
            if let Some(node) = nodes.iter_mut().find(|c| c.call_id == *call_id) {
                route(&mut node.children, event);
            }
        }
        AgentEvent::ToolStart {
            call_id,
            name,
            arguments,
        } => nodes.push(SubCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            result: None,
            children: Vec::new(),
        }),
        AgentEvent::ToolEnd { result } => {
            if let Some(node) = nodes.iter_mut().find(|c| c.call_id == result.call_id) {
                node.result = Some(result.clone());
            }
        }
        _ => {}
    }
}

/// A subagent's captured calls as live one-liners, each indented by its depth.
fn child_lines(subs: &[SubCall], depth: usize, width: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for sc in subs {
        lines.push(calls::step(&sc.name, &sc.arguments, depth).truncate(width));
        lines.extend(child_lines(&sc.children, depth + 1, width));
    }
    lines
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
    /// Whether a tool call has been pushed since the last reply boundary, so a reply's later
    /// calls stack under its first without a blank between them.
    batch_open: bool,
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
            batch_open: false,
        }
    }

    /// The window of the model whose reply ends next.
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

    /// Opens or closes the thinking block or subagent task at `index`; `false` when it is neither.
    pub fn toggle(&mut self, index: usize) -> bool {
        let togglable = matches!(
            self.entries.get(index).map(|e| &e.block),
            Some(Block::Thinking { .. })
        ) || (self.is_task(index) && self.has_children(index));
        if togglable && let Some(entry) = self.entries.get_mut(index) {
            entry.open = !entry.open;
            entry.drawn = None;
            return true;
        }
        false
    }

    /// Opens every thinking block and subagent task, or closes them all when all are open; new
    /// ones follow suit.
    pub fn toggle_all(&mut self) {
        let idxs: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                matches!(self.entries[i].block, Block::Thinking { .. })
                    || (self.is_task(i) && self.has_children(i))
            })
            .collect();
        let open = if idxs.is_empty() {
            !self.open_thinking
        } else {
            !idxs.iter().all(|&i| self.entries[i].open)
        };
        for &i in &idxs {
            self.entries[i].open = open;
            self.entries[i].drawn = None;
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
                // A reply's tools follow its MessageEnd; the next one starts a fresh batch.
                self.batch_open = false;
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
                children: Vec::new(),
            }),
            AgentEvent::ToolEnd { result } => {
                let removed = self
                    .running
                    .iter()
                    .position(|r| r.call_id == result.call_id)
                    .map(|i| self.running.remove(i));
                let (arguments, children) = removed
                    .map(|r| (r.arguments, r.children))
                    .unwrap_or_default();
                let first = !self.batch_open;
                self.batch_open = true;
                self.push(Block::Tool {
                    arguments,
                    result: result.clone(),
                    first,
                    depth: 0,
                });
                // A subagent's captured calls follow its task card, indented a level deeper.
                self.flatten(children, 1);
            }
            AgentEvent::Unsaved { path, error } => {
                let text = fill(&words::get().unsaved, &[("path", path), ("error", error)]);
                self.notice(&text, "warn");
            }
            AgentEvent::RunEnd(Ended { outcome, .. }) => {
                self.finish_reasoning(now);
                self.finish_text();
                self.running.clear();
                self.batch_open = false;
                let note = match outcome {
                    Outcome::Aborted => Some(words::get().interrupted.clone()),
                    Outcome::Rejected => Some(ui::get().text.declined.clone()),
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
            // A subagent's calls are captured under the task that started it, to show as its tree.
            AgentEvent::Child { call_id, event } => {
                if let Some(task) = self.running.iter_mut().find(|r| r.call_id == *call_id) {
                    route(&mut task.children, event);
                }
            }
            // The session words what an MCP server's event says, as a notice.
            AgentEvent::Retry { .. }
            | AgentEvent::Prompt { .. }
            | AgentEvent::SnapshotOff { .. }
            | AgentEvent::Mcp { .. } => {}
        }
    }

    /// Lays the transcript out at `width` and returns its height in rows: blocks one blank
    /// row apart, then the part still in flight.
    pub fn measure(&mut self, width: usize, now: Instant) -> usize {
        let mut row = 0;
        self.starts.clear();
        // Entries deeper than `floor` sit under a collapsed task and draw nothing.
        let mut floor = usize::MAX;
        for i in 0..self.entries.len() {
            let depth = self.entries[i].depth();
            if depth > floor {
                // A hidden entry shares its start with the row below, where lines kept from when
                // it was shown would otherwise draw.
                self.entries[i].drawn = None;
                self.starts.push(row);
                continue;
            }
            floor = if self.is_task(i) && !self.entries[i].open {
                depth
            } else {
                usize::MAX
            };
            let decor = Decor {
                depth,
                rail: self.batch_rail(i),
            };
            if i > 0 && !self.stacks_up(i) {
                row += 1;
            }
            self.starts.push(row);
            row += self.entries[i].lines(width, decor).len();
        }
        self.live = self.live_lines(width, now);
        if !self.live.is_empty() && !self.entries.is_empty() {
            row += 1;
        }
        self.live_start = row;
        self.total = row + self.live.len();
        self.total
    }

    /// A `task` whose subagent made calls; only these (and thinking) toggle open.
    fn is_task(&self, i: usize) -> bool {
        self.entries.get(i).is_some_and(
            |e| matches!(&e.block, Block::Tool { result, .. } if result.name == "task"),
        )
    }

    /// Whether entry `i` is followed by deeper (subagent-child) entries.
    fn has_children(&self, i: usize) -> bool {
        let depth = self.entries.get(i).map_or(0, Entry::depth);
        self.entries.get(i + 1).is_some_and(|e| e.depth() > depth)
    }

    /// Whether entry `i` draws directly under the one before it with no blank: a subagent's child,
    /// or a reply's batched call stacking under an adjacent sibling.
    fn stacks_up(&self, i: usize) -> bool {
        if self.entries.get(i).map_or(0, Entry::depth) >= 1 {
            return true;
        }
        let continues = matches!(
            self.entries.get(i).map(|e| &e.block),
            Some(Block::Tool { first: false, .. })
        );
        continues
            && i > 0
            && self
                .entries
                .get(i - 1)
                .is_some_and(|e| matches!(e.block, Block::Tool { .. }) && e.depth() == 0)
    }

    /// Whether tool `i` is one of ≥2 grouped calls of a reply, so it shares the batch rail.
    fn batch_rail(&self, i: usize) -> bool {
        let depth0_tool = |j: usize| {
            self.entries
                .get(j)
                .is_some_and(|e| matches!(e.block, Block::Tool { .. }) && e.depth() == 0)
        };
        let continues = |j: usize| {
            self.entries.get(j).is_some_and(|e| {
                matches!(e.block, Block::Tool { first: false, .. }) && e.depth() == 0
            })
        };
        // A batch when the next call continues it, or this call continues an adjacent one before it.
        // Children flattened between two task calls break adjacency, so those tasks share no rail.
        depth0_tool(i) && (continues(i + 1) || (continues(i) && i > 0 && depth0_tool(i - 1)))
    }

    /// Lays a subagent's captured calls, depth-first, into indented child entries after its task.
    fn flatten(&mut self, subs: Vec<SubCall>, depth: usize) {
        for sc in subs {
            let result = sc.result.unwrap_or_else(|| ToolResult {
                call_id: sc.call_id,
                name: sc.name,
                content: String::new(),
                is_error: false,
                details: None,
                asks: Vec::new(),
                refused: None,
                usage: None,
            });
            self.entries.push(Entry {
                block: Block::Tool {
                    arguments: sc.arguments,
                    result,
                    first: false,
                    depth,
                },
                open: false,
                drawn: None,
            });
            self.flatten(sc.children, depth + 1);
        }
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
                    .map_or(&[][..], |(_, _, l)| l.as_slice());
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
                        lines.extend(child_lines(&tool.children, 0, width));
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
        // Collapsed by default: the subagent's read is kept but hidden, the count stands in.
        assert_eq!(
            texts(&mut feed, now),
            ["● task find the parser", "  explore: 2 calls"]
        );
        // Toggling the task reveals its child, indented under a rail; toggling again hides it.
        assert!(feed.toggle(0));
        assert_eq!(
            texts(&mut feed, now),
            ["● task find the parser", "│ ● read src/parse.rs"]
        );
        assert!(feed.toggle(0));
        assert_eq!(
            texts(&mut feed, now),
            ["● task find the parser", "  explore: 2 calls"]
        );
        // Re-collapsed with a block below: the gap between them stays blank.
        feed.push(Block::User("next".into()));
        assert_eq!(texts(&mut feed, now)[2], "");
        assert!(feed.toggle(0));
        texts(&mut feed, now);
        assert!(feed.toggle(0));
        assert_eq!(
            texts(&mut feed, now)[2],
            "",
            "no hidden child draws in the gap"
        );
    }

    #[test]
    fn parallel_subagents_keep_each_tasks_children_together() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        let start = |id: &str, name: &str, args: &str| AgentEvent::ToolStart {
            call_id: id.into(),
            name: name.into(),
            arguments: args.into(),
        };
        let child = |task: &str, ev: AgentEvent| AgentEvent::Child {
            call_id: task.into(),
            event: Box::new(ev),
        };
        let task_end = |id: &str, calls: u64| AgentEvent::ToolEnd {
            result: ToolResult {
                call_id: id.into(),
                name: "task".into(),
                content: "done".into(),
                details: Some(serde_json::json!({"agent": "explore", "calls": calls})),
                is_error: false,
                asks: Vec::new(),
                refused: None,
                usage: None,
            },
        };
        // Two task calls in one reply; their child calls arrive interleaved as they run.
        feed.event(&start("t1", "task", r#"{"description":"first"}"#), now);
        feed.event(&start("t2", "task", r#"{"description":"second"}"#), now);
        feed.event(&child("t1", start("a1", "read", r#"{"path":"1a"}"#)), now);
        feed.event(&child("t2", start("b1", "read", r#"{"path":"2a"}"#)), now);
        feed.event(&child("t1", start("a2", "read", r#"{"path":"1b"}"#)), now);
        feed.event(&task_end("t1", 2), now);
        feed.event(&task_end("t2", 1), now);
        feed.toggle_all(); // open both tasks
        // Each task's children stay contiguous under it, not interleaved.
        assert_eq!(
            texts(&mut feed, now),
            [
                "● task first",
                "│ ● read 1a",
                "│ ● read 1b",
                "",
                "● task second",
                "│ ● read 2a",
            ]
        );
    }

    #[test]
    fn a_nested_subagent_indents_a_grandchild_and_collapse_nests() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        let start = |id: &str, name: &str, args: &str| AgentEvent::ToolStart {
            call_id: id.into(),
            name: name.into(),
            arguments: args.into(),
        };
        let child = |task: &str, ev: AgentEvent| AgentEvent::Child {
            call_id: task.into(),
            event: Box::new(ev),
        };
        // Outer task → inner task → a read, two levels deep.
        feed.event(&start("t1", "task", r#"{"description":"outer"}"#), now);
        feed.event(
            &child("t1", start("t2", "task", r#"{"description":"inner"}"#)),
            now,
        );
        feed.event(
            &child(
                "t1",
                child("t2", start("c3", "read", r#"{"path":"deep.rs"}"#)),
            ),
            now,
        );
        feed.event(
            &AgentEvent::ToolEnd {
                result: ToolResult {
                    call_id: "t1".into(),
                    name: "task".into(),
                    content: "done".into(),
                    details: Some(serde_json::json!({"agent": "explore", "calls": 1})),
                    is_error: false,
                    asks: Vec::new(),
                    refused: None,
                    usage: None,
                },
            },
            now,
        );
        // Collapsed: only the outer task.
        assert_eq!(texts(&mut feed, now), ["● task outer", "  explore: 1 call"]);
        // Open the outer: the inner task shows (depth 1), its grandchild still hidden.
        assert!(feed.toggle(0));
        assert_eq!(texts(&mut feed, now), ["● task outer", "│ ● task inner"]);
        // Open the inner (entry 1): the grandchild shows a level deeper.
        assert!(feed.toggle(1));
        assert_eq!(
            texts(&mut feed, now),
            ["● task outer", "│ ● task inner", "│ │ ● read deep.rs"]
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
    fn calls_batched_in_one_reply_stack_but_a_new_reply_keeps_its_blank() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new();
        let start = |id: &str, cmd: &str| AgentEvent::ToolStart {
            call_id: id.into(),
            name: "bash".into(),
            arguments: format!(r#"{{"command":"{cmd}"}}"#),
        };
        let done = |id: &str, out: &str| AgentEvent::ToolEnd {
            result: ToolResult {
                call_id: id.into(),
                name: "bash".into(),
                content: out.into(),
                is_error: false,
                details: None,
                asks: Vec::new(),
                refused: None,
                usage: None,
            },
        };
        feed.event(&delta(DeltaKind::Text, "Reading."), now);
        feed.event(&end(None, Finish::ToolCalls), now); // reply ends; its calls are one batch
        feed.event(&start("c1", "one"), now);
        feed.event(&done("c1", "1"), now);
        feed.event(&start("c2", "two"), now);
        feed.event(&done("c2", "2"), now); // same reply: stacks under c1, no blank
        feed.event(&end(None, Finish::ToolCalls), now); // a new, tool-only reply
        feed.event(&start("c3", "three"), now);
        feed.event(&done("c3", "3"), now); // new batch: blank above
        assert_eq!(
            texts(&mut feed, now),
            [
                "◆ Reading.",
                "",
                "│ ● bash one", // the two calls of one reply share a left rail
                "│   1",
                "│ ● bash two",
                "│   2",
                "",
                "● bash three", // a lone call of its own reply: no rail
                "  3",
            ]
        );
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
