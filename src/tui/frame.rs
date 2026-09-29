//! The two rules framing the message bar, in the mode's colour: the top one carries the mode's
//! label, the bottom one the model, effort, context use and cost, right-aligned (order in
//! data/ui.toml). When the terminal is narrow the dashes shrink before any item is cut.

use crate::effort;
use crate::mode::Mode;
use crate::text::styled::{Line, Style};
use crate::text::units;
use crate::tui::ui;

const RULE: &str = "─";
/// Dashes kept at each end of the bottom rule so it still reads as a rule.
const LEAD: usize = 2;
const TAIL: usize = 2;

pub struct Info<'a> {
    pub model: &'a str,
    pub effort: Option<&'a str>,
    /// Share of the context window the last request used.
    pub context_pct: Option<u64>,
    pub cost_micros: u64,
}

pub fn top(mode: &Mode, width: usize) -> Line {
    let style = Style::fg(&mode.color);
    let mut line = Line::styled(format!("{RULE}{RULE} "), style.clone());
    line.push(&mode.label, style.clone().bold());
    line.push(" ", style.clone());
    let used = line.width();
    line.push(RULE.repeat(width.saturating_sub(used)), style);
    line.truncate(width)
}

pub fn bottom(mode: &Mode, info: &Info<'_>, width: usize) -> Line {
    let style = Style::fg(&mode.color);
    // Dashes, a space, the items, a space, the tail.
    let room = width.saturating_sub(LEAD + TAIL + 2);
    let items = items(info);
    if room == 0 || items.spans.is_empty() {
        return Line::styled(RULE.repeat(width), style);
    }
    let items = items.truncate(room);
    let mut line = Line::styled(
        RULE.repeat(width - items.width() - TAIL - 2) + " ",
        style.clone(),
    );
    line.extend(items);
    line.push(format!(" {}", RULE.repeat(TAIL)), style);
    line
}

fn items(info: &Info<'_>) -> Line {
    let footer = &ui::get().footer;
    let mut line = Line::default();
    for item in &footer.items {
        let Some(part) = item_line(info, item) else {
            continue;
        };
        if !line.spans.is_empty() {
            line.push(&footer.separator, Style::fg("muted"));
        }
        line.extend(part);
    }
    line
}

fn item_line(info: &Info<'_>, item: &str) -> Option<Line> {
    let muted = Style::fg("muted");
    Some(match item {
        "model" => {
            let mut line = Line::styled(info.model, muted.clone());
            if let Some(id) = info.effort {
                let color = effort::get(id).map_or("muted", |l| l.color.as_str());
                line.push(format!(" {id}"), Style::fg(color));
            }
            line
        }
        "context" => Line::styled(format!("ctx {}%", info.context_pct?), muted),
        "cost" => Line::styled(
            format!(
                "{} {}",
                units::usd(info.cost_micros as f64 / 1e6),
                ui::get().text.cost_estimate
            ),
            muted,
        ),
        other => {
            debug_assert!(false, "unknown footer item `{other}` in data/ui.toml");
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode;

    fn info() -> Info<'static> {
        Info {
            model: "crow-2",
            effort: Some("high"),
            context_pct: Some(12),
            cost_micros: 42_000,
        }
    }

    fn manual() -> &'static Mode {
        mode::get("manual").unwrap()
    }

    #[test]
    fn the_top_rule_carries_the_mode_and_fills_the_width() {
        let line = top(manual(), 30);
        assert_eq!(line.text(), format!("── MANUAL {}", "─".repeat(20)));
        assert_eq!(line.width(), 30);
    }

    #[test]
    fn the_bottom_rule_right_aligns_the_items_and_shrinks_its_dashes_first() {
        let items = "crow-2 high · ctx 12% · $0.042 est";
        for width in [100, 60] {
            let line = bottom(manual(), &info(), width);
            assert_eq!(line.width(), width);
            assert!(
                line.text().ends_with(&format!(" {items} ──")),
                "{}",
                line.text()
            );
            assert!(line.text().starts_with("──"));
        }
        let narrow = bottom(manual(), &info(), 30);
        assert_eq!(narrow.width(), 30);
        assert!(
            narrow.text().starts_with("── crow-2 high"),
            "{}",
            narrow.text()
        );
        assert!(narrow.text().ends_with(" ──"));
        assert_eq!(bottom(manual(), &info(), 4).text(), "────");
    }

    #[test]
    fn the_frame_is_in_the_mode_colour_and_effort_in_its_level_colour() {
        let line = bottom(mode::get("plan").unwrap(), &info(), 60);
        let tagged = line.to_tagged();
        assert!(tagged.starts_with("[mode_plan]"), "{tagged}");
        assert!(tagged.contains("[effort_high] high[/]"), "{tagged}");
    }

    #[test]
    fn items_without_a_value_are_skipped() {
        let i = Info {
            effort: None,
            context_pct: None,
            ..info()
        };
        assert!(
            bottom(manual(), &i, 60)
                .text()
                .ends_with(" crow-2 · $0.042 est ──")
        );
    }
}
