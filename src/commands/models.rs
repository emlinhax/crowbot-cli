use std::fmt::Write;
use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::api::models::{self, Catalog, Model, Source};
use crate::text::table::Align;
use crate::text::template::fill;
use crate::text::units;

const SRC: &str = include_str!("../../data/commands/models.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static DATA: LazyLock<Data> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/commands/models.toml is checked by tests"));

#[derive(Deserialize)]
struct Data {
    drop: Vec<String>,
    column: Vec<Column>,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Column {
    id: String,
    title: String,
    #[serde(default)]
    align: Align,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    current: String,
    footnote: String,
    live: String,
    cache: String,
    snapshot: String,
    unreachable: String,
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
                    _ => bail!("usage: {}", SPEC.usage),
                }
            }
            if cx.scope == Scope::Session {
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::PickModel { refresh }],
                });
            }
            let catalog = models::load(cx.app, refresh).await;
            Ok(render(&catalog, &cx.app.settings.model).into())
        })
    }
}

pub fn titles() -> Vec<String> {
    DATA.column.iter().map(|c| c.title.clone()).collect()
}

pub fn aligns() -> Vec<Align> {
    DATA.column.iter().map(|c| c.align).collect()
}

/// Column positions to leave out when narrow, first to go first.
pub fn drop_order() -> Vec<usize> {
    DATA.drop
        .iter()
        .filter_map(|id| DATA.column.iter().position(|c| c.id == *id))
        .collect()
}

/// The mark beside the model in use.
pub fn current_mark() -> &'static str {
    &DATA.text.current
}

/// One model's cells, in column order.
pub fn cells(m: &Model) -> Vec<String> {
    DATA.column.iter().map(|c| cell(m, &c.id)).collect()
}

fn cell(m: &Model, column: &str) -> String {
    match column {
        "id" => m.id.clone(),
        "name" => m.display_name.clone(),
        "context" => units::tokens(m.context_window),
        "input" => units::usd(m.pricing.input_per_1m_usd),
        "cached" => units::usd(m.pricing.cached_input_per_1m_usd),
        "output" => units::usd(m.pricing.output_per_1m_usd),
        "max_output" => units::tokens(m.max_output_tokens),
        "capabilities" => [
            ("reasoning", m.capabilities.reasoning),
            ("tools", m.capabilities.tool_calling),
            ("cache", m.capabilities.caching),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" "),
        other => {
            debug_assert!(
                false,
                "unknown column `{other}` in data/commands/models.toml"
            );
            String::new()
        }
    }
}

/// Where the list came from, and why not from crowbot when it could not be reached.
pub fn source_note(catalog: &Catalog) -> String {
    let text = &DATA.text;
    let when = catalog
        .fetched_at
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default();
    let mut note = match catalog.source {
        Source::Live => text.live.clone(),
        Source::Cache => fill(&text.cache, &[("when", &when)]),
        Source::Snapshot => text.snapshot.clone(),
    };
    if let Some(why) = &catalog.note {
        note.push(' ');
        note.push_str(&fill(&text.unreachable, &[("note", why)]));
    }
    note
}

fn render(catalog: &Catalog, default_model: &str) -> String {
    let text = &DATA.text;
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
            cells[0] = format!("{} {}", cells[0], text.current);
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

    #[test]
    fn every_column_has_a_cell_and_the_table_lines_up() {
        let catalog = Catalog {
            models: models::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        for m in &catalog.models {
            assert_eq!(cells(m).len(), DATA.column.len());
        }
        assert_eq!(
            drop_order().len(),
            DATA.drop.len(),
            "drop names unknown columns"
        );
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
