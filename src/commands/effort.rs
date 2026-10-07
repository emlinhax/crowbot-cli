use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::data;
use crate::effort;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("effort");
static TEXT: LazyLock<Text> = data::load!("commands/effort.toml", "text");

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
                    effort::find(level)?;
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
