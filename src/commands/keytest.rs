use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Spec};
use crate::io;
use crate::io::term::{self, Event, KeyEvent, KeyEventKind};

const SRC: &str = include_str!("../../data/commands/keytest.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static STEPS: LazyLock<Steps> =
    LazyLock::new(|| toml::from_str(SRC).expect("keytest steps are checked by tests"));

/// Silence that ends a paste burst; generous, since this only measures.
const BURST_END: Duration = Duration::from_millis(400);

#[derive(Deserialize)]
struct Steps {
    step: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    label: String,
    prompt: String,
    /// Text to paste back; its presence makes this a paste step.
    #[serde(default)]
    sample: Option<String>,
}

pub struct KeyTest;

impl Command for KeyTest {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        _cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let only = args.first().cloned();
            if let Some(label) = &only
                && !STEPS
                    .step
                    .iter()
                    .any(|s| s.label.eq_ignore_ascii_case(label))
            {
                anyhow::bail!("usage: {}", SPEC.usage);
            }
            let report = tokio::task::spawn_blocking(move || probe(only.as_deref())).await??;
            Ok(format!(
                "Report (paste this back):\n\n```\n{}\n```",
                report.join("\n")
            )
            .into())
        })
    }
}

/// Runs every step, or only the one labelled `only`.
fn probe(only: Option<&str>) -> std::io::Result<Vec<String>> {
    let raw = term::Raw::enter()?;
    let mut report = vec![format!("keyboard enhancement: {}", raw.enhanced())];
    let steps = STEPS
        .step
        .iter()
        .filter(|s| only.is_none_or(|o| s.label.eq_ignore_ascii_case(o)));
    for step in steps {
        term::out(&format!("\r\n{}\r\n", step.prompt));
        if let Some(sample) = &step.sample {
            term::out(&format!("{}\r\n", sample.replace('\n', "\r\n")));
        }
        let got = match &step.sample {
            Some(_) => burst()?,
            None => first_press()?,
        };
        term::out(&format!("  got: {got}\r\n"));
        report.push(format!("{}: {got}", step.label));
    }
    drop(raw);
    term::out("\n");
    Ok(report)
}

fn first_press() -> std::io::Result<String> {
    Ok(match next_input()? {
        Event::Key(key) => describe(&key),
        Event::Paste(text) => format!("paste event ({} chars)", text.len()),
        other => format!("{other:?}"),
    })
}

/// The next key press or paste, skipping releases left over from the previous step.
fn next_input() -> std::io::Result<Event> {
    loop {
        match term::read_event()? {
            Event::Key(key) if key.kind == KeyEventKind::Release => {}
            event @ (Event::Key(_) | Event::Paste(_)) => return Ok(event),
            _ => {}
        }
    }
}

/// Collects one paste: everything from the first event until the input goes quiet.
fn burst() -> std::io::Result<String> {
    let mut first = next_input()?;
    let start = io::clock::instant();
    let (mut presses, mut enters, mut pastes) = (0, 0, 0);
    let mut last_press = start;
    let mut max_gap = Duration::ZERO;
    loop {
        match &first {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let now = io::clock::instant();
                if presses > 0 {
                    max_gap = max_gap.max(now - last_press);
                }
                last_press = now;
                presses += 1;
                if key.code == term::KeyCode::Enter {
                    enters += 1;
                }
            }
            Event::Paste(_) => pastes += 1,
            _ => {}
        }
        match term::poll_event(BURST_END)? {
            Some(next) => first = next,
            None => break,
        }
    }
    Ok(format!(
        "{presses} key presses ({enters} Enter), {pastes} paste events, {}ms total, max gap {}ms",
        (last_press - start).as_millis(),
        max_gap.as_millis()
    ))
}

fn describe(key: &KeyEvent) -> String {
    format!("{:?} {:?} {:?}", key.code, key.modifiers, key.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_parse() {
        assert!(STEPS.step.iter().any(|s| s.sample.is_some()));
        assert!(STEPS.step.iter().all(|s| !s.label.is_empty()));
    }
}
