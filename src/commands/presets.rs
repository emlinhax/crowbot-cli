use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::data;
use crate::io::term;
use crate::models::preset::{self, Preset};
use crate::models::preset_table::{folder_note, header, row, rule_rows};
use crate::models::{self, choice};
use crate::text::table;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("presets");
static TEXT: LazyLock<Text> = data::load!("commands/presets.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    written: String,
}

pub struct Presets;

impl Command for Presets {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let name = match args {
                [] => None,
                [new] if new == "new" => Some(None),
                [new, name] if new == "new" => Some(Some(name.clone())),
                _ => return Err(cx.usage(&SPEC)),
            };
            let paths = &cx.app.paths;
            match (cx.scope, name) {
                (Scope::Session, None) => Ok(Effect::PickPreset.into()),
                (Scope::Session, Some(name)) => Ok(Effect::NewPreset { name }.into()),
                (Scope::Cli, None) => {
                    let listed = preset::load(paths);
                    let current = &cx.app.settings.model;
                    Ok(render(&listed, current, &paths.presets(), term::out_width()).into())
                }
                (Scope::Cli, Some(name)) => {
                    // The new preset's rule names a model; the default, or what its preset falls
                    // back on.
                    let catalog = models::load(&cx.app.api, paths, false).await;
                    let chosen =
                        choice::find(&catalog, &preset::load(paths), &cx.app.settings.model);
                    let model =
                        chosen.map_or(cx.app.settings.model.clone(), |c| c.model().id.clone());
                    let path = preset::create(paths, name.as_deref(), &model)?;
                    let id = path.file_stem().unwrap_or_default().to_string_lossy();
                    let path = path.display().to_string();
                    Ok(fill(&TEXT.written, &[("path", &path), ("id", &id)]).into())
                }
            }
        })
    }
}

/// Each preset with its rules under it, in `width` cells, then where they live.
fn render(listed: &[Preset], current: &str, dir: &std::path::Path, width: usize) -> String {
    let mut rows = vec![header()];
    for preset in listed {
        rows.push(row(preset, current));
        rows.extend(rule_rows(preset));
    }
    let mut out = match listed.is_empty() {
        true => String::new(),
        false => table::plain_text(&rows, &[], &[], width) + "\n",
    };
    out.push_str(&folder_note(dir, listed.is_empty()));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preset_lists_its_rules_under_it_and_the_folder_follows() {
        let listed = [preset::parsed(
            "cheap",
            "description = \"flash\"\n[[rule]]\nwhen = { agent = [\"subagent\"] }\nmodel = \"a\"\n[[rule]]\nmodel = \"b\"",
        )];
        let shown = render(&listed, "cheap", "/h/presets".as_ref(), usize::MAX);
        let lines: Vec<&str> = shown.lines().collect();
        assert!(
            lines[1].starts_with("●") && lines[1].contains("cheap"),
            "{shown}"
        );
        assert!(
            lines[2].contains("agent subagent") && lines[2].contains("→ a"),
            "{shown}"
        );
        assert!(
            lines[3].contains("otherwise") && lines[3].contains("→ b"),
            "{shown}"
        );
        assert!(shown.contains("/h/presets"), "{shown}");
        let none = render(&[], "m", "/h/presets".as_ref(), usize::MAX);
        assert!(none.starts_with("No presets yet."), "{none}");
    }
}
