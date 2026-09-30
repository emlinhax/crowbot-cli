//! Where a streaming reply can be cut: the finished blocks before the point can be rendered
//! once and kept; what follows may still change as the reply arrives.

/// The byte length of `text` made of finished blocks: everything up to the last blank line that is
/// not inside a code fence. The rest may still change as the reply streams in.
pub fn complete_prefix(text: &str) -> usize {
    let mut fence: Option<String> = None;
    let mut cut = 0;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let end = at + line.len();
        let trimmed = line.trim_start();
        if let Some(open) = &fence {
            if line.ends_with('\n')
                && trimmed.trim_end().starts_with(open.as_str())
                && trimmed
                    .trim()
                    .chars()
                    .all(|c| c == open.as_bytes()[0] as char)
            {
                fence = None;
            }
        } else if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
        } else if line.trim().is_empty() && line.ends_with('\n') {
            cut = end;
        }
        at = end;
    }
    cut
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

fn fence_marker(line: &str) -> Option<String> {
    let c = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let run = line.chars().take_while(|x| *x == c).count();
    (run >= 3).then(|| c.to_string().repeat(run))
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
    fn half_arrived_fences_are_hidden() {
        assert_eq!(trim_partial_fence("code\n``"), "code\n");
        assert_eq!(trim_partial_fence("code\nmore"), "code\nmore");
    }
}
