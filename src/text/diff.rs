//! A unified diff as coloured lines.

use crate::text::styled::{Line, Style};

/// Every body line of `diff`, coloured and cut to `width`; how many to show is the caller's.
pub fn render(diff: &str, width: usize) -> Vec<Line> {
    // Only a unified diff's own two header lines go: inside a hunk a removed `-- note` reads
    // `--- note`, and a preview of new content has no headers at all.
    let mut body: Vec<&str> = diff.lines().collect();
    if body.len() >= 2 && body[0].starts_with("--- ") && body[1].starts_with("+++ ") {
        body.drain(..2);
    }
    body.iter()
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_changes() {
        let diff = "--- a\n+++ a\n@@ -1 +1 @@\n-old\n+new\n context\n";
        let lines: Vec<String> = render(diff, 80).iter().map(Line::to_tagged).collect();
        assert_eq!(
            lines,
            vec![
                "[diff_hunk]@@ -1 +1 @@[/]",
                "[diff_del]-old[/]",
                "[diff_add]+new[/]",
                "[dim] context[/]",
            ]
        );
    }

    #[test]
    fn removed_lines_that_look_like_headers_still_show() {
        let diff = "--- a/q.sql\n+++ b/q.sql\n@@ -1,3 +1,2 @@\n--- keep me\n+++i;\n----\n";
        let lines: Vec<String> = render(diff, 80).iter().map(Line::text).collect();
        assert_eq!(
            lines,
            vec!["@@ -1,3 +1,2 @@", "--- keep me", "+++i;", "----"]
        );
    }

    #[test]
    fn a_preview_without_headers_is_shown_whole() {
        let lines: Vec<String> = render("+++ x\n+y\n", 80).iter().map(Line::text).collect();
        assert_eq!(lines, vec!["+++ x", "+y"]);
        let lines: Vec<String> = render("$ ls -la", 80).iter().map(Line::text).collect();
        assert_eq!(lines, vec!["$ ls -la"]);
    }
}
