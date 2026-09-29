//! The line above the bar while crowbot works: a raven flapping, a verb with a glint running
//! along it, then time, tokens and how to stop. A retry says why and how long instead.

use std::time::Instant;

use unicode_segmentation::UnicodeSegmentation;

use crate::agent::event::AgentEvent;
use crate::limits;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::text::units;
use crate::tui::ui;

/// CEILING: the tokens of a reply still streaming are estimated at this many characters each;
/// the exact count replaces it when the request ends. A tokenizer would make it exact.
const CHARS_PER_TOKEN: usize = 4;
/// Steps the glint spends off the word before its next pass, so it reads as a glint, not a scroll.
const GLINT_REST: usize = 6;

/// One turn's progress, from the moment it is sent.
pub struct Progress {
    started: Instant,
    verb: usize,
    /// Output tokens of this turn's finished requests.
    output: u64,
    /// Characters streamed by the request in flight.
    streaming: usize,
}

impl Progress {
    /// Picks a verb, never `last` again when there is another.
    pub fn start(now: Instant, last: Option<usize>, rng: &mut fastrand::Rng) -> Self {
        let count = ui::get().status.verbs.len().max(1);
        let mut verb = rng.usize(..count);
        if Some(verb) == last && count > 1 {
            verb = (verb + 1 + rng.usize(..count - 1)) % count;
        }
        Self {
            started: now,
            verb,
            output: 0,
            streaming: 0,
        }
    }

    pub fn verb(&self) -> usize {
        self.verb
    }

    pub fn event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Delta { text, .. } => self.streaming += text.len(),
            AgentEvent::MessageEnd { message } => {
                self.output += message.usage.map_or(0, |u| u.output);
                self.streaming = 0;
            }
            _ => {}
        }
    }

    pub fn tokens(&self) -> u64 {
        self.output + (self.streaming / CHARS_PER_TOKEN) as u64
    }

    /// `color` is the mode's; `braille` picks the raven over plain dots.
    pub fn render(&self, now: Instant, color: &str, braille: bool, width: usize) -> Line {
        let words = &ui::get().status;
        let limits = &limits::get().tui;
        let elapsed = now.duration_since(self.started).as_millis();
        let frames = if braille { &words.wings } else { &words.plain };
        let step = (elapsed / u128::from(limits.wing_ms.value.max(1))) as usize;
        let frame = frames
            .get(step % frames.len().max(1))
            .map_or("", String::as_str);
        let verb = words.verbs.get(self.verb).map_or("", String::as_str);

        let mut line = Line::styled(format!("{frame} "), Style::fg(color));
        line.extend(glint(
            &format!("{verb}…"),
            elapsed,
            limits.shimmer_ms.value,
            color,
        ));
        let tokens = fill(&words.tokens, &[("n", &units::tokens(self.tokens()))]);
        let text = &ui::get().text;
        line.push(
            format!("  {}s · {tokens} · {}", elapsed / 1000, text.interrupt),
            Style::fg("muted"),
        );
        line.truncate(width)
    }
}

pub fn retrying(secs: u64, why: &str, width: usize) -> Line {
    let text = &ui::get().text;
    let muted = Style::fg("muted");
    let mut line = Line::styled(format!("{why} · "), Style::fg("warn"));
    line.push(format!("{} {secs}s", text.retrying), muted.clone());
    line.push(format!(" · {}", text.interrupt), muted);
    line.truncate(width)
}

/// `word` muted, with a glint (bright centre, edges in the mode colour) where `elapsed` has
/// swept it to.
fn glint(word: &str, elapsed_ms: u128, step_ms: u64, color: &str) -> Line {
    let letters: Vec<&str> = word.graphemes(true).collect();
    // The centre starts one letter before the word and leaves one after, then rests.
    let pass = letters.len() + 2 + GLINT_REST;
    let at = (elapsed_ms / u128::from(step_ms.max(1))) as usize % pass;
    let mut line = Line::default();
    for (i, letter) in letters.iter().enumerate() {
        let style = match (i + 1).abs_diff(at) {
            0 => Style::fg("strong").bold(),
            1 => Style::fg(color).bold(),
            _ => Style::fg("muted"),
        };
        line.push(*letter, style);
    }
    line
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::agent::event::DeltaKind;
    use crate::agent::message::{Assistant, Finish, Usage};

    fn progress(verb: usize) -> (Progress, Instant) {
        let now = crate::io::clock::instant();
        let mut p = Progress::start(now, None, &mut fastrand::Rng::with_seed(1));
        p.verb = verb;
        (p, now)
    }

    #[test]
    fn the_line_has_a_raven_a_verb_and_the_stats() {
        let (p, t0) = progress(2);
        let line = p.render(t0, "mode_auto", true, 80);
        assert_eq!(line.text(), "⠑⠤⠊ Cooking…  0s · ↓ 0 tok · esc to interrupt");
        let wing = limits::get().tui.wing_ms.value;
        let later = p.render(t0 + Duration::from_millis(wing), "mode_auto", true, 80);
        assert!(later.text().starts_with("⠢⠤⠔ "), "{}", later.text());
        let plain = p.render(t0, "mode_auto", false, 80);
        assert!(plain.text().starts_with("· Cooking…"));
    }

    #[test]
    fn the_glint_moves_along_the_verb() {
        let step = limits::get().tui.shimmer_ms.value;
        let bright = |ms: u64| {
            glint("Cooking…", u128::from(ms), step, "mode_auto")
                .spans
                .iter()
                .position(|s| s.style == Style::fg("strong").bold())
        };
        // Step 1 lights the first letter, step 3 the third; off the word, nothing is lit.
        let first = bright(step);
        let third = bright(3 * step);
        assert!(first.is_some() && third > first, "{first:?} {third:?}");
        assert_eq!(bright(0), None);
    }

    #[test]
    fn a_verb_never_repeats_twice_running() {
        let mut rng = fastrand::Rng::with_seed(7);
        let now = crate::io::clock::instant();
        let mut last = None;
        for _ in 0..200 {
            let p = Progress::start(now, last, &mut rng);
            assert_ne!(Some(p.verb()), last);
            last = Some(p.verb());
        }
    }

    #[test]
    fn tokens_are_estimated_while_streaming_then_exact() {
        let (mut p, _) = progress(0);
        p.event(&AgentEvent::Delta {
            kind: DeltaKind::Text,
            text: "x".repeat(400),
            index: None,
            name: None,
        });
        assert_eq!(p.tokens(), 100);
        p.event(&AgentEvent::MessageEnd {
            message: Assistant {
                parts: Vec::new(),
                model: "m".into(),
                effort: None,
                usage: Some(Usage {
                    input: 10,
                    cached: 0,
                    output: 250,
                    reasoning: 0,
                    cost_micros: 0,
                }),
                finish: Finish::Done,
                error: None,
                request_id: None,
            },
        });
        assert_eq!(p.tokens(), 250);
    }
}
