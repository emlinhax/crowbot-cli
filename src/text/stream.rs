//! Where a streaming reply can be cut: the finished blocks before the point can be rendered
//! once and kept; what follows may still change as the reply arrives.

use pulldown_cmark::{Event, Parser};

use crate::text::markdown;

/// The byte length of `text` made of finished blocks: everything before the last top-level
/// block, which may still be growing (a list gains items, a fence its closing line). Parsed as
/// the renderer parses, so a cut never falls inside a block it draws whole.
pub fn complete_prefix(text: &str) -> usize {
    let mut depth = 0usize;
    let mut last = 0;
    for (event, range) in Parser::new_ext(text, markdown::options()).into_offset_iter() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    last = range.start;
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            // A block with no start and end of its own, like a rule.
            _ if depth == 0 => last = range.start,
            _ => {}
        }
    }
    // Back to the start of that block's line, so its indent stays with it.
    text[..last].rfind('\n').map_or(0, |i| i + 1)
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
    fn half_arrived_fences_are_hidden() {
        assert_eq!(trim_partial_fence("code\n``"), "code\n");
        assert_eq!(trim_partial_fence("code\nmore"), "code\nmore");
    }
}
