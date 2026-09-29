//! The line under the editor: mode, model, effort, context use and cost, dropping the least
//! important items first when the terminal is narrow (order and drop order in data/ui.toml).

use crate::text::styled::{Line, Style};
use crate::text::units;
use crate::tui::ui;

pub struct State<'a> {
    pub mode_label: &'a str,
    pub mode_color: &'a str,
    pub model: &'a str,
    /// Crow models are shown in their own colour.
    pub crow: bool,
    pub effort: Option<&'a str>,
    /// Share of the context window the last request used.
    pub context_pct: Option<u64>,
    pub cost_micros: u64,
}

pub fn render(state: &State<'_>, width: usize) -> Line {
    let footer = &ui::get().footer;
    let mut shown: Vec<&String> = footer.items.iter().collect();
    let mut drops = footer.drop.iter();
    loop {
        let line = join(state, &shown);
        if line.width() <= width {
            return line;
        }
        match drops.next() {
            Some(drop) => shown.retain(|item| *item != drop),
            None => return line.truncate(width),
        }
    }
}

fn join(state: &State<'_>, items: &[&String]) -> Line {
    let footer = &ui::get().footer;
    let mut line = Line::default();
    for item in items {
        let Some(part) = item_line(state, item) else {
            continue;
        };
        if !line.spans.is_empty() {
            line.push(&footer.separator, Style::fg("muted"));
        }
        line.extend(part);
    }
    line
}

fn item_line(state: &State<'_>, item: &str) -> Option<Line> {
    let muted = Style::fg("muted");
    Some(match item {
        "mode" => {
            let mut line = Line::styled(state.mode_label, Style::fg(state.mode_color).bold());
            line.push(format!(" {}", ui::get().footer.mode_hint), muted);
            line
        }
        "model" => Line::styled(
            state.model,
            if state.crow {
                Style::fg("crow")
            } else {
                Style::default()
            },
        ),
        "effort" => Line::styled(state.effort?, muted),
        "context" => Line::styled(format!("ctx {}%", state.context_pct?), muted),
        "cost" => Line::styled(
            format!(
                "{} {}",
                units::usd(state.cost_micros as f64 / 1e6),
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

    fn state() -> State<'static> {
        State {
            mode_label: "MANUAL",
            mode_color: "mode_manual",
            model: "crow-2",
            crow: true,
            effort: Some("high"),
            context_pct: Some(12),
            cost_micros: 42_000,
        }
    }

    #[test]
    fn shows_everything_when_wide_and_drops_in_order_when_narrow() {
        let wide = render(&state(), 80).text();
        assert_eq!(wide, "MANUAL ⇧⇥ · crow-2 · high · ctx 12% · $0.042 est");
        assert_eq!(
            render(&state(), 40).text(),
            "MANUAL ⇧⇥ · crow-2 · high · ctx 12%"
        );
        assert_eq!(render(&state(), 25).text(), "MANUAL ⇧⇥ · crow-2");
        assert!(render(&state(), 6).width() <= 6);
    }

    #[test]
    fn items_without_a_value_are_skipped() {
        let s = State {
            effort: None,
            context_pct: None,
            ..state()
        };
        assert_eq!(render(&s, 80).text(), "MANUAL ⇧⇥ · crow-2 · $0.042 est");
    }
}
