//! `crowbot battery report <dir> [--against <dir>]`: the tool-use battery's report (src/battery/).

use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::battery::{load, report};
use crate::config::settings;
use crate::data;
use crate::io::{self, fs::Access};
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("battery");
static WORDS: LazyLock<report::Words> = data::load!("commands/battery.toml");

pub struct Battery;

impl Command for Battery {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let words: Vec<&str> = args.iter().map(String::as_str).collect();
            let (dir, against) = match words.as_slice() {
                ["report", dir] => (*dir, None),
                ["report", dir, "--against", other] => (*dir, Some(*other)),
                _ => return Err(cx.usage(&SPEC)),
            };
            let at = |d: &str| cx.app.paths.project.join(d);
            let this = load::load(&at(dir))?;
            let that = against.map(|d| load::load(&at(d))).transpose()?;
            // Battery runs start from a fresh home, so the built-in rules are the ones they had.
            let text = report::render(&this, that.as_ref(), &WORDS, &settings::default_rules());
            let path = at(dir).join("report.md");
            io::fs::write_atomic(&path, text.as_bytes(), Access::Shared)?;
            let saved = fill(&WORDS.text.saved, &[("path", &path.display().to_string())]);
            Ok(format!("{text}\n{saved}").into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_parse_and_every_replayed_mode_exists() {
        assert!(WORDS.top > 0 && WORDS.candidates > 0);
        assert!(!WORDS.smell.is_empty());
        for mode in &WORDS.replay {
            assert!(crate::mode::get(mode).is_some(), "{mode}");
        }
        for smell in &WORDS.smell {
            assert!(
                !smell.words.is_empty() || smell.contains.is_some(),
                "{}",
                smell.label
            );
        }
    }
}
