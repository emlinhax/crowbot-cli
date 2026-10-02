use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/init.toml")));

const PROMPT: &str = include_str!("../../data/prompts/init.md");

pub struct Init;

impl Command for Init {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&SPEC, args)?;
            Ok(Outcome {
                text: String::new(),
                effects: vec![Effect::Send(PROMPT.trim_end().to_owned())],
            })
        })
    }
}
