//! Messages typed while crowbot works, shown until they are delivered.

use crate::text::styled::{Line, Style};
use crate::tui::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Delivered when the run would otherwise stop.
    Queued,
    /// Delivered after the current tool calls.
    Steering,
}

pub fn render(entries: &[(Kind, String)], width: usize) -> Vec<Line> {
    let text = &ui::get().text;
    entries
        .iter()
        .map(|(kind, message)| {
            let label = match kind {
                Kind::Queued => &text.queued,
                Kind::Steering => &text.steering,
            };
            let first = message.lines().next().unwrap_or_default();
            let mut line = Line::styled(format!("↳ {label}: "), Style::fg("muted"));
            line.push(first, Style::default().dim());
            line.truncate(width)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_per_entry() {
        let lines = render(
            &[
                (Kind::Queued, "then run the tests\nand more".into()),
                (Kind::Steering, "use trash instead".into()),
            ],
            80,
        );
        let texts: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(
            texts,
            vec![
                "↳ queued: then run the tests",
                "↳ steering: use trash instead"
            ]
        );
    }
}
