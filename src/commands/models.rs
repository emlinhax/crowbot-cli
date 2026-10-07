use std::fmt::Write;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::data;
use crate::models::table::{aligns, cells, source_note, titles};
use crate::models::{self, Catalog};
use crate::text::table::Align;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("models");
static TEXT: LazyLock<Text> = data::load!("commands/models.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    footnote: String,
    missing: String,
}

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
                    _ => return Err(cx.usage(&SPEC)),
                }
            }
            if cx.scope == Scope::Session {
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::PickModel { refresh }],
                });
            }
            let catalog = models::load(&cx.app.api, &cx.app.paths, refresh).await;
            Ok(render(&catalog, &cx.app.settings.model).into())
        })
    }
}

fn render(catalog: &Catalog, default_model: &str) -> String {
    let text = &*TEXT;
    let row = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let mut out = row(titles());
    let rule = aligns()
        .iter()
        .map(|a| match a {
            Align::Left => "---",
            Align::Right => "--:",
        })
        .collect::<Vec<_>>()
        .join("|");
    let _ = writeln!(out, "|{rule}|");
    for m in &catalog.models {
        let mut cells = cells(m);
        if m.id == default_model {
            cells[0] = format!("{} {}", cells[0], models::table::current_mark());
        }
        out.push_str(&row(cells));
    }
    let _ = writeln!(out, "\n{} {}", text.footnote, source_note(catalog));
    if !catalog.models.iter().any(|m| m.id == default_model) {
        let _ = writeln!(out, "{}", fill(&text.missing, &[("model", default_model)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Source;

    #[test]
    fn the_table_lines_up() {
        let catalog = Catalog {
            models: models::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        let md = render(&catalog, "crow-2");
        let lines = crate::text::markdown::render(&md, 200);
        let widths: Vec<usize> = lines
            .iter()
            .filter(|l| l.text().starts_with('│'))
            .map(|l| l.width())
            .collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}
