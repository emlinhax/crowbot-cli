//! The claim tool: a subagent asks for files outside its scope. Granted when no other subagent
//! holds them; otherwise it hears who does, and with `wait` waits for them to finish
//! (agent/claims.rs). It asks no one: the table answers for the agent that started the subagents,
//! which is busy waiting on them.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::agent::claims;

/// The tool's name: a subagent that can change files is given it, the agent the user works with
/// never is.
pub const NAME: &str = "claim";

static SPEC: LazyLock<Spec> = crate::tools::spec!("claim");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    paths: Vec<String>,
    #[serde(default)]
    wait: bool,
}

pub struct Claim;

impl Tool for Claim {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        match args.paths.iter().all(|p| p.trim().is_empty()) {
            true => Err(Refusal::InvalidArgs("`paths` is empty".into())),
            false => Ok(Check::new(Vec::new())),
        }
    }

    /// A wait ends with the interrupt, which the table watches itself.
    fn stops_on_cancel(&self) -> bool {
        true
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let paths: Vec<String> = args
                .paths
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| claims::normalize(p))
                .collect();
            let run = cx.run.ok_or_else(claims::not_subagent)?;
            run.claim(&paths, args.wait).await.map(Output::ok)
        })
    }
}
