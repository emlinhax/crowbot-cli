use std::sync::LazyLock;

use anyhow::anyhow;
use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::mode;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/mode.toml")));

pub struct Mode;

impl Command for Mode {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let effect = match args {
                [] => Effect::CycleMode,
                [id] => Effect::SetMode(mode::find(id).map_err(|e| anyhow!(e))?.id.clone()),
                _ => return Err(cx.usage(&SPEC)),
            };
            Ok(Outcome {
                text: String::new(),
                effects: vec![effect],
            })
        })
    }
}
