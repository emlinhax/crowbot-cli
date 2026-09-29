//! The one rounded box every floating card is drawn in: prompt cards and the command popup.

use crate::text::styled::{Line, Style};

/// `title` on the top edge, `body` padded to the inner width, all within `width` cells.
pub fn boxed(title: Line, body: &[Line], width: usize) -> Vec<Line> {
    let border = Style::fg("muted");
    let inner = width.saturating_sub(4);
    let mut lines = Vec::with_capacity(body.len() + 2);
    let mut top = Line::styled("╭ ", border.clone());
    top.extend(title);
    top = top.truncate(width.saturating_sub(2));
    top.push(" ", border.clone());
    let fill = width.saturating_sub(top.width() + 1);
    top.push(format!("{}╮", "─".repeat(fill)), border.clone());
    lines.push(top);
    for row in body {
        let row = row.truncate(inner);
        let mut line = Line::styled("│ ", border.clone());
        let pad = inner.saturating_sub(row.width());
        line.extend(row);
        line.push(format!("{} │", " ".repeat(pad)), border.clone());
        lines.push(line);
    }
    lines.push(Line::styled(
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        border,
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_exactly_the_width() {
        let body = [
            Line::plain("short"),
            Line::plain("a row far too long to fit inside the box"),
        ];
        let lines = boxed(Line::plain("title"), &body, 24);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(text[0], format!("╭ title {}╮", "─".repeat(15)));
        assert_eq!(text[1], "│ short                │");
        assert!(text[2].ends_with("… │"), "{}", text[2]);
        assert_eq!(text[3], format!("╰{}╯", "─".repeat(22)));
        assert!(lines.iter().all(|l| l.width() == 24));
    }
}
