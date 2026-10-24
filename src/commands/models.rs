use std::fmt::Write;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::agent::roles::RoleId;
use crate::data;
use crate::io::term;
use crate::models::table::{aligns, drop_order, mark, rows, source_note};
use crate::models::{self, Catalog};
use crate::text::table::{self, GAP};
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("models");
static TEXT: LazyLock<Text> = data::load!("commands/models.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    footnote: String,
    missing: String,
    unknown_role: String,
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
            let mut role = None;
            for arg in args {
                match arg.as_str() {
                    "--refresh" => refresh = true,
                    // A role names what the session's card opens on; the list has none.
                    named if role.is_none() && cx.scope == Scope::Session => {
                        role = Some(RoleId::parse(named).ok_or_else(|| unknown_role(named))?);
                    }
                    _ => return Err(cx.usage(&SPEC)),
                }
            }
            if cx.scope == Scope::Session {
                return Ok(Effect::PickModel { refresh, role }.into());
            }
            let catalog = models::load(&cx.app.api, &cx.app.paths, refresh).await;
            // Piped, the table keeps every column; a terminal gets what fits it.
            let width = if term::stdout_is_terminal() {
                term::size().0
            } else {
                usize::MAX
            };
            Ok(render(&catalog, &cx.app.settings.model, width).into())
        })
    }
}

fn unknown_role(named: &str) -> anyhow::Error {
    let roles: Vec<String> = RoleId::all()
        .iter()
        .map(|r| format!("`{}`", r.id()))
        .collect();
    anyhow::anyhow!(fill(
        &TEXT.unknown_role,
        &[("role", named), ("roles", &roles.join(", "))]
    ))
}

/// The table laid out in `width` cells, then where the list came from.
fn render(catalog: &Catalog, default_model: &str, width: usize) -> String {
    let rows = table::plain(
        &rows(catalog, default_model),
        &aligns(),
        &drop_order(),
        width,
        GAP,
    );
    let mut out = String::new();
    for row in rows {
        let _ = writeln!(out, "{}", row.text().trim_end());
    }
    let footnote = fill(&TEXT.footnote, &[("current", mark())]);
    let _ = writeln!(out, "\n{footnote} {}", source_note(catalog));
    if !catalog.models.iter().any(|m| m.id == default_model) {
        let _ = writeln!(out, "{}", fill(&TEXT.missing, &[("model", default_model)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Source;

    #[test]
    fn the_table_lines_up_with_the_default_marked() {
        let catalog = Catalog {
            models: models::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        let shown = render(&catalog, "crow-2", usize::MAX);
        let lines: Vec<&str> = shown.lines().take_while(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), catalog.models.len() + 1, "{shown}");
        let column = |line: &str, word: &str| line.find(word).map(|i| line[..i].chars().count());
        let model = column(lines[0], "model");
        for (line, m) in lines[1..].iter().zip(&catalog.models) {
            assert_eq!(column(line, &m.id), model, "{line}");
        }
        assert!(lines.iter().any(|l| l.starts_with("●")), "{shown}");
        assert!(!shown.contains('|'), "{shown}");
    }
}
