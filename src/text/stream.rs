//! Where a streaming reply can be cut: the finished blocks before the point can be rendered
//! once and kept; what follows may still change as the reply arrives.

use pulldown_cmark::{Event, Parser};

use crate::text::markdown;

/// The byte length of `text` made of finished blocks: everything before the last top-level
/// block that follows a blank line. What comes after may still be growing (a list gains items,
/// a fence its closing line), and a block with no blank line before it may still turn out to be
/// part of the one above: half a table row parses as a paragraph until its cells arrive. Parsed
/// as the renderer parses, so a cut never falls inside a block it draws whole.
pub fn complete_prefix(text: &str) -> usize {
    let mut depth = 0usize;
    let mut last = 0;
    for (event, range) in Parser::new_ext(text, markdown::options()).into_offset_iter() {
        let top = match event {
            Event::Start(_) => {
                depth += 1;
                depth == 1
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                false
            }
            // A block with no start and end of its own, like a rule.
            _ => depth == 0,
        };
        if top {
            last = after_blank(text, range.start).unwrap_or(last);
        }
    }
    last
}

/// The start of the line `at` is on, when the line above it is blank; the start of the line
/// keeps a block's indent with it.
fn after_blank(text: &str, at: usize) -> Option<usize> {
    let line = text[..at].rfind('\n')? + 1;
    let above = &text[..line - 1];
    let above = &above[above.rfind('\n').map_or(0, |i| i + 1)..];
    above.trim().is_empty().then_some(line)
}

/// The byte length of the blank lines `text` opens with; an indent on its first real line stays.
pub fn blank_lead(text: &str) -> usize {
    let blank = text.len() - text.trim_start().len();
    text[..blank].rfind('\n').map_or(0, |i| i + 1)
}

/// Hides a closing fence that has only partly arrived, so a code block does not flicker shut.
pub fn trim_partial_fence(tail: &str) -> &str {
    let Some(start) = tail.rfind('\n').map(|i| i + 1).or(Some(0)) else {
        return tail;
    };
    let last = tail[start..].trim_start();
    if !last.is_empty() && last.chars().all(|c| c == '`' || c == '~') {
        &tail[..start]
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_cuts_at_blank_lines_outside_fences() {
        let text = "para one\n\n```\ncode\n\nmore code\n";
        assert_eq!(&text[..complete_prefix(text)], "para one\n\n");
        let closed = "```\nx\n\n```\n\nnext";
        assert_eq!(&closed[..complete_prefix(closed)], "```\nx\n\n```\n\n");
    }

    #[test]
    fn a_cut_never_splits_a_list_an_indented_block_or_a_list_fence() {
        for text in ["- ```\n  code\n\n  more\n  ```\n", "- a\n\n  still a\n"] {
            assert_eq!(complete_prefix(text), 0, "{text:?}");
        }
        let indented = "para\n\n    a\n\n    b\n";
        assert_eq!(&indented[..complete_prefix(indented)], "para\n\n");
    }

    #[test]
    fn a_cut_waits_for_a_blank_line_so_a_block_can_still_join_the_one_above() {
        // The `|` is the next row arriving; parsed alone it would be a paragraph.
        let table = "| a | b |\n|---|---|\n|";
        assert_eq!(complete_prefix(table), 0);
        let after = "Intro\n\n| a | b |\n|---|---|\n| 1";
        assert_eq!(&after[..complete_prefix(after)], "Intro\n\n");
        for text in ["# Title\nMore", "para\n---", "> quote\nlazy"] {
            assert_eq!(complete_prefix(text), 0, "{text:?}");
        }
    }

    #[test]
    fn leading_blank_lines_are_measured_up_to_the_first_real_line() {
        assert_eq!(blank_lead("\n \n\nAnswer"), 4);
        assert_eq!(blank_lead("\n    code"), 1);
        assert_eq!(blank_lead("  "), 0);
        assert_eq!(blank_lead("Answer\n\n"), 0);
    }

    #[test]
    fn half_arrived_fences_are_hidden() {
        assert_eq!(trim_partial_fence("code\n``"), "code\n");
        assert_eq!(trim_partial_fence("code\nmore"), "code\nmore");
    }
}
