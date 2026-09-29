use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/quit.toml")));

pub struct Quit;

impl Command for Quit {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        _cx: &'a Ctx<'a>,
        _args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async {
            Ok(Outcome {
                text: String::new(),
                effects: vec![Effect::Quit],
            })
        })
    }
}
