//! The conversation as it appears on screen. Agent events arrive here; finished blocks come out
//! as lines to commit, and whatever is still in flight is the live part.

use std::time::{Duration, Instant};

use crate::agent::event::{AgentEvent, DeltaKind, Outcome};
use crate::agent::message::{Assistant, Finish};
use crate::limits;
use crate::text::markdown;
use crate::text::styled::{Line, Style};
use crate::tui::cards::{self, State};
use crate::tui::ui;

struct Running {
    call_id: String,
    name: String,
    arguments: String,
    since: Instant,
}

pub struct Feed {
    width: usize,
    commits: Vec<Line>,
    /// A block was committed, so the next one starts after a blank line.
    spaced: bool,
    /// The assistant text of the reply in flight, and how much of it is committed.
    text: String,
    committed: usize,
    reasoning: String,
    reasoning_since: Option<Instant>,
    show_reasoning: bool,
    running: Vec<Running>,
    retry: Option<(Instant, String)>,
}

impl Feed {
    pub fn new(width: usize) -> Self {
        Self {
            width,
            commits: Vec::new(),
            spaced: false,
            text: String::new(),
            committed: 0,
            reasoning: String::new(),
            reasoning_since: None,
            show_reasoning: false,
            running: Vec::new(),
            retry: None,
        }
    }

    pub fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    /// Lines ready to go into scrollback, oldest first.
    pub fn take_commits(&mut self) -> Vec<Line> {
        std::mem::take(&mut self.commits)
    }

    /// Commits lines exactly as given (the welcome screen).
    pub fn raw(&mut self, lines: Vec<Line>) {
        self.commits.extend(lines);
    }

    pub fn user(&mut self, text: &str) {
        let prompt = Line::styled(&ui::get().prompt, Style::fg("user").bold());
        let indent = Line::plain(" ".repeat(prompt.width()));
        let mut lines = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let mut line = if i == 0 {
                prompt.clone()
            } else {
                indent.clone()
            };
            line.push(raw, Style::default());
            lines.extend(line.wrap(self.width, &indent));
        }
        self.block(lines);
    }

    pub fn markdown(&mut self, md: &str) {
        self.block(markdown::render(md, self.width));
    }

    pub fn notice(&mut self, text: &str, role: &str) {
        self.block(Line::styled(text, Style::fg(role)).wrap(self.width, &Line::default()));
    }

    /// Whether reasoning is committed in full from now on.
    pub fn toggle_reasoning(&mut self) -> bool {
        self.show_reasoning = !self.show_reasoning;
        self.show_reasoning
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
                let card = cards::finished(&result.name, &arguments, result, self.width);
                self.block(card);
            }
            AgentEvent::Retry {
                delay_ms, error, ..
            } => {
                let until = now + Duration::from_millis(*delay_ms);
                self.retry = Some((until, error.entry().title.clone()));
            }
            AgentEvent::Notice { text } => self.notice(text, "warn"),
            AgentEvent::RunEnd { outcome } => {
                self.finish_reasoning(now);
                self.finish_text();
                self.running.clear();
                self.retry = None;
                let note = match outcome {
                    Outcome::Aborted => Some(&ui::get().text.interrupted),
                    Outcome::Rejected => Some(&ui::get().text.declined),
                    _ => None,
                };
                if let Some(note) = note {
                    self.notice(note, "muted");
                }
            }
            AgentEvent::Ask { .. } => {}
        }
    }

    /// The part still in flight: the reply's unfinished tail, the thinking tail, running tools.
    pub fn live(&self, now: Instant) -> Vec<Line> {
        let mut lines = Vec::new();
        if !self.reasoning.is_empty() {
            let tail_len = limits::get().tui.reasoning_tail_lines.value;
            let wrapped: Vec<Line> = Line::styled(
                self.reasoning.replace('\n', " "),
                Style::fg("reasoning").italic(),
            )
            .wrap(self.width, &Line::default());
            let skip = wrapped.len().saturating_sub(tail_len);
            lines.extend(wrapped.into_iter().skip(skip));
        }
        let tail = markdown::trim_partial_fence(&self.text[self.committed..]);
        if !tail.trim().is_empty() {
            if self.committed > 0 || self.spaced {
                lines.push(Line::default());
            }
            lines.extend(markdown::render(tail, self.width));
        }
        let limits = &limits::get().tui;
        for tool in &self.running {
            let elapsed = now.duration_since(tool.since).as_millis();
            let frame = ui::get().spinner_frame(elapsed, limits.spinner_ms.value);
            lines.push(
                cards::header(&tool.name, &tool.arguments, State::Running(frame))
                    .truncate(self.width),
            );
        }
        lines
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

    fn block(&mut self, lines: Vec<Line>) {
        if lines.is_empty() {
            return;
        }
        if self.spaced {
            self.commits.push(Line::default());
        }
        self.commits.extend(lines);
        self.spaced = true;
    }

    fn commit_complete(&mut self) {
        let pending = &self.text[self.committed..];
        let cut = markdown::complete_prefix(pending);
        if cut > 0 {
            let chunk = pending[..cut].to_owned();
            self.committed += cut;
            self.block(markdown::render(&chunk, self.width));
        }
    }

    fn finish_text(&mut self) {
        let rest = self.text[self.committed..].to_owned();
        if !rest.trim().is_empty() {
            self.block(markdown::render(&rest, self.width));
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
        let reasoning = std::mem::take(&mut self.reasoning);
        self.reasoning_since = None;
        let style = Style::fg("reasoning").italic();
        if self.show_reasoning {
            let lines = reasoning
                .lines()
                .flat_map(|l| Line::styled(l, style.clone()).wrap(self.width, &Line::default()))
                .collect();
            self.block(lines);
        } else {
            let text = format!("∴ {} {secs}s", ui::get().text.thought);
            self.block(vec![Line::styled(text, style)]);
        }
    }

    fn outcome_of(&mut self, message: &Assistant) {
        if let Some(error) = &message.error {
            let entry = error.entry();
            let mut lines = vec![Line::styled(
                format!("● {}", entry.title),
                Style::fg("error").bold(),
            )];
            let indent = Line::plain("  ");
            let mut detail = indent.clone();
            detail.push(&error.message, Style::default().dim());
            lines.extend(detail.wrap(self.width, &indent));
            if let Some(hint) = &entry.hint {
                let mut line = indent.clone();
                line.push(hint, Style::fg("muted"));
                lines.extend(line.wrap(self.width, &indent));
            }
            if let Some(id) = &error.request_id {
                let mut line = indent.clone();
                line.push(format!("request {id}"), Style::fg("muted"));
                lines.push(line);
            }
            self.block(lines);
        } else if message.finish == Finish::Length {
            let text = &ui::get().text.cut_off;
            self.notice(text, "warn");
        }
    }
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

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn finished_paragraphs_commit_while_the_tail_stays_live() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new(60);
        feed.event(&delta(DeltaKind::Text, "First para"), now);
        assert!(feed.take_commits().is_empty());
        assert_eq!(texts(&feed.live(now)), vec!["First para"]);
        feed.event(&delta(DeltaKind::Text, "graph.\n\nSecond"), now);
        assert_eq!(texts(&feed.take_commits()), vec!["First paragraph."]);
        assert_eq!(texts(&feed.live(now)), vec!["", "Second"]);
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(texts(&feed.take_commits()), vec!["", "Second"]);
        assert!(feed.live(now).is_empty());
    }

    #[test]
    fn reasoning_collapses_to_one_line_unless_shown() {
        let t0 = crate::io::clock::instant();
        let mut feed = Feed::new(60);
        feed.event(&delta(DeltaKind::Reasoning, "let me think"), t0);
        assert_eq!(texts(&feed.live(t0)), vec!["let me think"]);
        feed.event(
            &delta(DeltaKind::Text, "Answer"),
            t0 + Duration::from_secs(4),
        );
        assert_eq!(texts(&feed.take_commits()), vec!["∴ thought 4s"]);
        feed.toggle_reasoning();
        feed.event(&delta(DeltaKind::Reasoning, "more"), t0);
        feed.event(&end(None, Finish::Done), t0);
        let committed = texts(&feed.take_commits());
        assert!(committed.contains(&"more".to_owned()), "{committed:?}");
    }

    #[test]
    fn tools_run_live_and_commit_as_cards() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new(60);
        feed.event(
            &AgentEvent::ToolStart {
                call_id: "c1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            now,
        );
        assert!(feed.live(now)[0].text().ends_with("bash ls"));
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
        assert!(feed.live(now).is_empty());
        assert_eq!(texts(&feed.take_commits()), vec!["● bash ls", "  a.txt"]);
    }

    #[test]
    fn errors_become_cards_with_hint_and_request_id() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new(80);
        let error = ErrorInfo {
            request_id: Some("req_9".into()),
            ..ErrorInfo::local("insufficient_balance", "top up")
        };
        feed.event(&end(Some(error), Finish::Error), now);
        let lines = texts(&feed.take_commits());
        assert_eq!(lines[0], "● Out of funds");
        assert!(lines.iter().any(|l| l.contains("req_9")));
    }

    #[test]
    fn blocks_are_separated_by_one_blank_line() {
        let now = crate::io::clock::instant();
        let mut feed = Feed::new(60);
        feed.user("hi");
        feed.event(&delta(DeltaKind::Text, "hello"), now);
        feed.event(&end(None, Finish::Done), now);
        assert_eq!(texts(&feed.take_commits()), vec!["› hi", "", "hello"]);
    }
}
