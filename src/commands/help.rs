use std::fmt::Write;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Scope, Spec, available};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/help.toml")));

pub struct Help;

impl Command for Help {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        _args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { Ok(render(cx.scope).into()) })
    }
}

fn render(scope: Scope) -> String {
    let (intro, prefix) = match scope {
        Scope::Cli => ("Commands (`crowbot <command>`):\n\n", ""),
        Scope::Session => ("Commands:\n\n", "/"),
    };
    let mut out = String::from(intro);
    for spec in available(scope).map(|c| c.spec()).filter(|s| !s.hidden) {
        let _ = writeln!(out, "- `{prefix}{}` — {}", spec.usage, spec.summary);
    }
    out
}
