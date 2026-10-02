use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::effort;
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/effort.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/effort.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    current: String,
    unset: String,
}

pub struct Effort;

impl Command for Effort {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            match args {
                [] => {
                    let current = cx.session.as_ref().and_then(|s| s.effort.as_deref());
                    let levels: Vec<&str> =
                        effort::levels().iter().map(|l| l.id.as_str()).collect();
                    Ok(fill(
                        &TEXT.current,
                        &[
                            ("effort", current.unwrap_or(&TEXT.unset)),
                            ("levels", &levels.join(", ")),
                        ],
                    )
                    .into())
                }
                [level] => {
                    effort::validate(level)?;
                    Ok(Outcome {
                        text: String::new(),
                        effects: vec![Effect::SetEffort(level.clone())],
                    })
                }
                _ => Err(cx.usage(&SPEC)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usage_offers_every_level() {
        for level in effort::levels() {
            assert!(
                SPEC.usage.contains(&level.id),
                "{} is not in the usage",
                level.id
            );
        }
    }
}
