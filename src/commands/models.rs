use std::fmt::Write;
use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::api::models::{self, Catalog, Source};
use crate::text::units;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/models.toml")));

pub struct Models;

impl Command for Models {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let mut refresh = false;
            for arg in args {
                match arg.as_str() {
                    "--refresh" => refresh = true,
                    _ => bail!("usage: {}", SPEC.usage),
                }
            }
            let catalog = models::load(cx.app, refresh).await;
            Ok(render(&catalog, &cx.app.settings.model).into())
        })
    }
}

fn render(catalog: &Catalog, default_model: &str) -> String {
    let mut out = String::from(
        "| model | name | in | cached in | out | context | max out | |\n|---|---|---|---|---|---|---|---|\n",
    );
    for m in &catalog.models {
        let mark = if m.id == default_model { " ●" } else { "" };
        let caps = [
            ("reasoning", m.capabilities.reasoning),
            ("tools", m.capabilities.tool_calling),
            ("cache", m.capabilities.caching),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" ");
        let _ = writeln!(
            out,
            "| `{}`{mark} | {} | {} | {} | {} | {} | {} | {caps} |",
            m.id,
            m.display_name,
            units::usd(m.pricing.input_per_1m_usd),
            units::usd(m.pricing.cached_input_per_1m_usd),
            units::usd(m.pricing.output_per_1m_usd),
            units::tokens(m.context_window),
            units::tokens(m.max_output_tokens),
        );
    }
    let _ = write!(out, "\nPrices are USD per 1M tokens; ● is your default. ");
    let when = catalog
        .fetched_at
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default();
    let _ = match catalog.source {
        Source::Live => writeln!(out, "Live list from crowbot."),
        Source::Cache => writeln!(out, "Cached list from {when} (`--refresh` to update)."),
        Source::Snapshot => writeln!(out, "Offline: showing the list bundled with this build."),
    };
    if let Some(note) = &catalog.note {
        let _ = writeln!(out, "Could not reach crowbot: {note}");
    }
    if !catalog.models.iter().any(|m| m.id == default_model) {
        let _ = writeln!(
            out,
            "Your default model `{default_model}` is not in this list; pick another with `--model`."
        );
    }
    out
}
