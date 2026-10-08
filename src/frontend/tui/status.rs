//! The line above the bar while crowbot works: a murmuration of crows, a verb with a glint running
//! along it, then time, tokens and how to stop. A retry says why and how long instead.

use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use crate::api::error::Entry;
use crate::config::limits;
use crate::frontend::tui::ui;
use crate::frontend::words;
use crate::live::event::AgentEvent;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::text::units;

/// CEILING: the tokens of a reply still streaming are estimated at this many characters each;
/// the exact count replaces it when the request ends. A tokenizer would make it exact.
const CHARS_PER_TOKEN: usize = 4;

/// One turn's progress, from the moment it is sent.
pub struct Progress {
    started: Instant,
    verb: usize,
    /// Output tokens of this turn's finished requests.
    output: u64,
    /// Characters streamed by the request in flight.
    streaming: usize,
    /// A request waiting to be retried: when, and why.
    retry: Option<(Instant, String)>,
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
            retry: None,
        }
    }

    pub fn verb(&self) -> usize {
        self.verb
    }

    pub fn event(&mut self, event: &AgentEvent, now: Instant) {
        match event {
            AgentEvent::Delta { text, .. } => {
                self.retry = None;
                self.streaming += text.len();
            }
            AgentEvent::MessageEnd { message } => {
                self.retry = None;
                self.output += message.usage.map_or(0, |u| u.output);
                self.streaming = 0;
            }
            AgentEvent::Retry {
                delay_ms, error, ..
            } => {
                let until = now + Duration::from_millis(*delay_ms);
                self.retry = Some((until, Entry::of(error).title.clone()));
            }
            _ => {}
        }
    }

    pub fn tokens(&self) -> u64 {
        self.output + (self.streaming / CHARS_PER_TOKEN) as u64
    }

    /// The working line, or the retry's while one is pending. `color` is the mode's; `braille`
    /// picks the braille flock over plain characters.
    pub fn render(&self, now: Instant, color: &str, braille: bool, width: usize) -> Line {
        if let Some((until, why)) = &self.retry {
            let secs = until.saturating_duration_since(now).as_secs() + 1;
            return retrying(secs, why, width);
        }
        let words = &ui::get().status;
        let limits = &limits::get().tui;
        let elapsed = now.duration_since(self.started).as_millis();
        let frames = if braille { &words.frames } else { &words.plain };
        let frame = frame(frames, elapsed, limits.status_frame_ms.value.get());
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

/// The frame of `frames` that `elapsed_ms` of work has reached at `step_ms` a frame.
pub fn frame(frames: &[String], elapsed_ms: u128, step_ms: u64) -> &str {
    let at = steps(elapsed_ms, step_ms) % frames.len().max(1);
    frames.get(at).map_or("", String::as_str)
}

/// How many `step_ms` steps `elapsed_ms` holds.
fn steps(elapsed_ms: u128, step_ms: u64) -> usize {
    (elapsed_ms / u128::from(step_ms.max(1))) as usize
}

fn retrying(secs: u64, why: &str, width: usize) -> Line {
    let text = &ui::get().text;
    let muted = Style::fg("muted");
    let mut line = Line::styled(format!("{why} · "), Style::fg("warn"));
    line.push(format!("{} {secs}s", words::get().retrying), muted.clone());
    line.push(format!(" · {}", text.interrupt), muted);
    line.truncate(width)
}

/// `word` muted, with a glint (bright centre, edges in the mode colour) where `elapsed` has
/// swept it to.
fn glint(word: &str, elapsed_ms: u128, step_ms: u64, color: &str) -> Line {
    let letters: Vec<&str> = word.graphemes(true).collect();
    // The centre starts one letter before the word and leaves one after, then rests.
    let pass = letters.len() + 2 + limits::get().tui.glint_rest.value;
    let at = steps(elapsed_ms, step_ms) % pass;
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
    use crate::conversation::message::{Assistant, Finish, Usage};
    use crate::live::event::DeltaKind;

    fn progress(verb: usize) -> (Progress, Instant) {
        let now = crate::io::clock::instant();
        let mut p = Progress::start(now, None, &mut fastrand::Rng::with_seed(1));
        p.verb = verb;
        (p, now)
    }

    #[test]
    fn the_line_has_the_flock_a_verb_and_the_stats() {
        let (p, t0) = progress(2);
        let frames = &ui::get().status.frames;
        let line = p.render(t0, "mode_auto", true, 80);
        assert_eq!(
            line.text(),
            format!("{} Cooking…  0s · ↓ 0 tok · esc to interrupt", frames[0])
        );
        let step = limits::get().tui.status_frame_ms.value.get();
        let later = p.render(t0 + Duration::from_millis(step), "mode_auto", true, 80);
        assert!(
            later.text().starts_with(&format!("{} ", frames[1])),
            "{}",
            later.text()
        );
        let plain = p.render(t0, "mode_auto", false, 80);
        let first = &ui::get().status.plain[0];
        assert!(plain.text().starts_with(&format!("{first} Cooking…")));
    }

    #[test]
    fn a_retry_holds_the_line_until_the_reply_streams() {
        let (mut p, t0) = progress(2);
        let retry = AgentEvent::Retry {
            attempt: 1,
            delay_ms: 2500,
            error: crate::conversation::error::ErrorInfo::local("overloaded", "busy"),
        };
        p.event(&retry, t0);
        let waiting = p.render(t0, "mode_auto", true, 80).text();
        assert!(
            waiting.contains(&format!("{} 3s", words::get().retrying)),
            "{waiting}"
        );
        let delta = AgentEvent::Delta {
            kind: DeltaKind::Text,
            text: "x".into(),
            index: None,
            name: None,
        };
        p.event(&delta, t0);
        assert!(
            p.render(t0, "mode_auto", true, 80)
                .text()
                .contains("Cooking")
        );
    }

    #[test]
    fn every_frame_is_as_wide_as_the_first_so_the_verb_stays_put() {
        let status = &ui::get().status;
        for frames in [&status.frames, &status.plain] {
            let first = crate::text::styled::width(&frames[0]);
            assert!(
                frames
                    .iter()
                    .all(|f| crate::text::styled::width(f) == first),
                "{frames:?}"
            );
        }
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
        let (mut p, t0) = progress(0);
        let delta = AgentEvent::Delta {
            kind: DeltaKind::Text,
            text: "x".repeat(400),
            index: None,
            name: None,
        };
        p.event(&delta, t0);
        assert_eq!(p.tokens(), 100);
        let end = AgentEvent::MessageEnd {
            message: Assistant {
                usage: Some(Usage {
                    input: 10,
                    cached: 0,
                    output: 250,
                    reasoning: 0,
                    cost_micros: 0,
                }),
                ..Assistant::stub(Finish::Done, Vec::new())
            },
        };
        p.event(&end, t0);
        assert_eq!(p.tokens(), 250);
    }
}
