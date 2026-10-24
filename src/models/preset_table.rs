//! Presets as table rows: under the models in the session's picker, and with their rules in
//! `/presets` and `crowbot presets` (data/preset_table.toml).

use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;
use crate::models::preset::{Preset, Rule, When};
use crate::models::table;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::text::units;

static TEXT: LazyLock<Text> = data::load!("preset_table.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    heading: String,
    preset: String,
    rules: String,
    new: String,
    always: String,
    otherwise: String,
    mode: String,
    agent: String,
    context_over: String,
    allowance_under: String,
    balance_under: String,
    joined: String,
    or: String,
    picks: String,
    picks_effort: String,
    folder: String,
    none: String,
}

/// The header of a table of presets alone.
pub fn header() -> Vec<Line> {
    let title = |t: &str| Line::styled(t, Style::fg("muted").bold());
    vec![Line::default(), title(&TEXT.preset), title(&TEXT.rules)]
}

/// The row that sets the presets apart from the models above them.
pub fn heading() -> Vec<Line> {
    vec![
        Line::default(),
        Line::styled(&TEXT.heading, Style::fg("muted").bold()),
    ]
}

/// A preset's row: the mark when it is in use, its name, then what it is for, or why it cannot
/// be chosen.
pub fn row(preset: &Preset, current: &str) -> Vec<Line> {
    let mark = match preset.id == current {
        true => Line::styled(table::mark(), Style::fg("accent")),
        false => Line::default(),
    };
    let about = match &preset.problem {
        Some(problem) => Line::styled(problem, Style::fg("warn")),
        None => Line::plain(&preset.description),
    };
    vec![mark, Line::plain(&preset.id), about]
}

/// A row under the preset's for each of its rules: when it holds, and what it picks.
pub fn rule_rows(preset: &Preset) -> Vec<Vec<Line>> {
    let alone = preset.rules.len() == 1;
    preset
        .rules
        .iter()
        .map(|rule| {
            let muted = Style::fg("muted");
            vec![
                Line::default(),
                Line::styled(format!("  {}", when(&rule.when, alone)), muted.clone()),
                Line::styled(picks(rule), muted),
            ]
        })
        .collect()
}

/// The last row, which writes a new preset to edit.
pub fn new_row() -> Vec<Line> {
    vec![Line::default(), Line::styled(&TEXT.new, Style::fg("muted"))]
}

/// Where presets live and how to make one; `empty` says there are none yet.
pub fn folder_note(dir: &Path, empty: bool) -> String {
    let note = fill(&TEXT.folder, &[("dir", &dir.display().to_string())]);
    match empty {
        true => format!("{} {note}", TEXT.none),
        false => note,
    }
}

/// A rule's conditions in words; one with none is the catch-all, or the only rule.
fn when(when: &When, alone: bool) -> String {
    let ids = |ids: &[String]| ids.join(&TEXT.or);
    let mut parts = Vec::new();
    if !when.mode.is_empty() {
        parts.push(fill(&TEXT.mode, &[("ids", &ids(&when.mode))]));
    }
    if !when.agent.is_empty() {
        parts.push(fill(&TEXT.agent, &[("ids", &ids(&when.agent))]));
    }
    if let Some(tokens) = when.context_over {
        let tokens = units::tokens(tokens);
        parts.push(fill(&TEXT.context_over, &[("tokens", &tokens)]));
    }
    if let Some(pct) = when.allowance_under_pct {
        let pct = pct.to_string();
        parts.push(fill(&TEXT.allowance_under, &[("pct", &pct)]));
    }
    if let Some(usd) = when.balance_under_usd {
        parts.push(fill(&TEXT.balance_under, &[("usd", &units::usd(usd))]));
    }
    match (parts.is_empty(), alone) {
        (true, true) => TEXT.always.clone(),
        (true, false) => TEXT.otherwise.clone(),
        (false, _) => parts.join(&TEXT.joined),
    }
}

fn picks(rule: &Rule) -> String {
    match &rule.effort {
        Some(effort) => fill(
            &TEXT.picks_effort,
            &[("model", &rule.model), ("effort", effort)],
        ),
        None => fill(&TEXT.picks, &[("model", &rule.model)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::preset;

    #[test]
    fn rules_read_as_their_conditions_then_the_catch_all() {
        let p = preset::parsed(
            "cheap",
            r#"
            description = "flash for reads"
            [[rule]]
            when = { mode = ["plan", "auto"], agent = ["subagent"], context_over = 200000 }
            model = "a"
            effort = "low"
            [[rule]]
            when = { allowance_under_pct = 20, balance_under_usd = 5 }
            model = "b"
            [[rule]]
            model = "c"
            "#,
        );
        let text = |row: &[Line]| row.iter().map(Line::text).collect::<Vec<_>>();
        assert_eq!(
            text(&row(&p, "cheap")),
            [table::mark(), "cheap", "flash for reads"]
        );
        let rules: Vec<Vec<String>> = rule_rows(&p).iter().map(|r| text(r)).collect();
        assert_eq!(
            rules,
            [
                [
                    "",
                    "  mode plan or auto, agent subagent, context over 200K",
                    "→ a, effort low"
                ],
                ["", "  allowance under 20%, balance under $5.00", "→ b"],
                ["", "  otherwise", "→ c"],
            ]
        );
        let alone = preset::parsed("one", "[[rule]]\nmodel = \"c\"");
        assert_eq!(text(&rule_rows(&alone)[0])[1], "  always");
    }

    #[test]
    fn a_preset_that_cannot_be_chosen_shows_why_in_its_row() {
        let p = preset::parsed("bad", "description = \"x\"");
        let row = row(&p, "other");
        assert!(row[2].text().contains("no [[rule]]"), "{}", row[2].text());
        assert_eq!(row[0].text(), "");
    }
}
