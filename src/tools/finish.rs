//! The finish tool: the model calls it when the task is complete, ideally in the same batch as its
//! last action or verifying command. The run loop ends the run after that batch only if every other
//! call in it succeeded (see `src/agent/run.rs`); if one failed, the model sees the error and goes
//! on, so an optimistic finish never ends a run early.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};

/// The name the run loop matches to tell a finish call from the others in a batch.
pub const NAME: &str = "finish";

static SPEC: LazyLock<Spec> = crate::tools::spec!("finish");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[serde(default)]
    summary: Option<String>,
}

pub struct Finish;

impl Tool for Finish {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, _args: &Self::Args, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(Check::new(Vec::new()))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        _cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move { Ok(Output::ok(args.summary.unwrap_or_default())) })
    }
}
