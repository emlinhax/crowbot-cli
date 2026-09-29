//! A unified diff as coloured lines.

use crate::text::styled::{Line, Style};

/// At most `max` lines of `diff`, each cut to `width`; a final line counts what was left out.
pub fn render(diff: &str, width: usize, max: usize) -> Vec<Line> {
    let body: Vec<&str> = diff
        .lines()
        .filter(|l| !l.starts_with("+++") && !l.starts_with("---"))
        .collect();
    let mut out: Vec<Line> = body
        .iter()
        .take(max)
        .map(|l| {
            let role = match l.chars().next() {
                Some('+') => Some("diff_add"),
                Some('-') => Some("diff_del"),
                Some('@') => Some("diff_hunk"),
                _ => None,
            };
            let style = role
                .map(Style::fg)
                .unwrap_or_else(|| Style::default().dim());
            Line::styled(*l, style).truncate(width)
        })
        .collect();
    if body.len() > max {
        let rest = body.len() - max;
        out.push(Line::styled(
            format!("… {rest} more line{}", if rest == 1 { "" } else { "s" }),
            Style::fg("muted"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_changes_and_counts_the_rest() {
        let diff = "--- a\n+++ a\n@@ -1 +1 @@\n-old\n+new\n context\n+more\n";
        let lines: Vec<String> = render(diff, 80, 4).iter().map(Line::to_tagged).collect();
        assert_eq!(
            lines,
            vec![
                "[diff_hunk]@@ -1 +1 @@[/]",
                "[diff_del]-old[/]",
                "[diff_add]+new[/]",
                "[dim] context[/]",
                "[muted]… 1 more line[/]",
            ]
        );
    }
}
