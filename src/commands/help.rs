use std::fmt::Write;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Scope, Spec, available};
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/help.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/help.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    heading: String,
    item: String,
    aliases: String,
}

pub struct Help;

impl Command for Help {
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
            Ok(render(cx.scope).into())
        })
    }
}

fn render(scope: Scope) -> String {
    let mut out = format!("{}\n\n", TEXT.heading);
    for spec in available(scope).map(|c| c.spec()).filter(|s| !s.hidden) {
        let aliases = if spec.aliases.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = spec
                .aliases
                .iter()
                .map(|a| format!("`{}`", scope.invoke(a)))
                .collect();
            fill(&TEXT.aliases, &[("names", &names.join(", "))])
        };
        let item = fill(
            &TEXT.item,
            &[
                ("usage", &scope.invoke(&spec.usage)),
                ("aliases", &aliases),
                ("summary", &spec.summary),
            ],
        );
        let _ = writeln!(out, "{item}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_names_each_command_with_its_aliases() {
        let help = render(Scope::Session);
        assert!(help.contains("- `/new` (or `/clear`) — "), "{help}");
        assert!(help.contains("- `/quit` (or `/exit`) — "), "{help}");
        assert!(!render(Scope::Cli).contains("/new"));
    }
}
