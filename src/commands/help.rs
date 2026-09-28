use std::fmt::Write;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{COMMANDS, Command, Spec};
use crate::app::App;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/help.toml")));

pub struct Help;

impl Command for Help {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        _app: &'a App,
        _args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async { Ok(render()) })
    }
}

fn render() -> String {
    let mut out = String::from("Commands (also usable as `/name` inside a session):\n\n");
    for spec in COMMANDS.iter().map(|c| c.spec()).filter(|s| !s.hidden) {
        let _ = writeln!(out, "- `{}` — {}", spec.usage, spec.summary);
    }
    out
}
