use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Spec};
use crate::io;
use crate::io::term::{self, Input, KeyEvent};

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
            let report = probe(only.as_deref()).await?;
            Ok(format!(
                "Report (paste this back):\n\n```\n{}\n```",
                report.join("\n")
            )
            .into())
        })
    }
}

/// Runs every step, or only the one labelled `only`.
async fn probe(only: Option<&str>) -> std::io::Result<Vec<String>> {
    let raw = term::Raw::enter()?;
    let mut inputs = std::pin::pin!(term::inputs());
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
            Some(_) => burst(&mut inputs).await?,
            None => first_press(&mut inputs).await?,
        };
        term::out(&format!("  got: {got}\r\n"));
        report.push(format!("{}: {got}", step.label));
    }
    drop(raw);
    term::out("\n");
    Ok(report)
}

async fn first_press(inputs: &mut (impl Stream<Item = Input> + Unpin)) -> std::io::Result<String> {
    Ok(match next_input(inputs).await? {
        Input::Key(key) => describe(&key),
        Input::Paste(text) => format!("paste event ({} chars)", text.len()),
        other => format!("{other:?}"),
    })
}

/// The next key press or paste.
async fn next_input(inputs: &mut (impl Stream<Item = Input> + Unpin)) -> std::io::Result<Input> {
    while let Some(input) = inputs.next().await {
        if matches!(input, Input::Key(_) | Input::Paste(_)) {
            return Ok(input);
        }
    }
    Err(std::io::ErrorKind::UnexpectedEof.into())
}

/// Collects one paste: everything from the first input until the input goes quiet.
async fn burst(inputs: &mut (impl Stream<Item = Input> + Unpin)) -> std::io::Result<String> {
    let mut input = next_input(inputs).await?;
    let start = io::clock::instant();
    let (mut presses, mut enters, mut pastes) = (0, 0, 0);
    let mut last_press = start;
    let mut max_gap = Duration::ZERO;
    loop {
        match &input {
            Input::Key(key) => {
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
            Input::Paste(_) => pastes += 1,
            _ => {}
        }
        match tokio::time::timeout(BURST_END, inputs.next()).await {
            Ok(Some(next)) => input = next,
            _ => break,
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
