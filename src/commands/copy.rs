use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/copy.toml")));

pub struct Copy;

impl Command for Copy {
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
                effects: vec![Effect::CopyLastReply],
            })
        })
    }
}
