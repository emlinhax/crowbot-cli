//! Keeps tool output inside the line and byte budget, whole lines only.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The beginning: files read top-down.
    Head,
    /// The end: command output, where errors and results land.
    Tail,
}

pub struct Cut {
    pub text: String,
    pub total_lines: usize,
    /// How many whole lines were kept.
    pub kept_lines: usize,
}

impl Cut {
    pub fn truncated(&self) -> bool {
        self.kept_lines < self.total_lines
    }
}

pub fn cut(text: &str, keep: Keep, max_lines: usize, max_bytes: usize) -> Cut {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0;
    let ordered: Box<dyn Iterator<Item = &&str>> = match keep {
        Keep::Head => Box::new(lines.iter()),
        Keep::Tail => Box::new(lines.iter().rev()),
    };
    for line in ordered {
        if kept.len() >= max_lines || bytes + line.len() + 1 > max_bytes {
            break;
        }
        bytes += line.len() + 1;
        kept.push(line);
    }
    // One line bigger than the whole budget still shows its start rather than nothing.
    if kept.is_empty() && !lines.is_empty() {
        let first = match keep {
            Keep::Head => lines[0],
            Keep::Tail => lines[total_lines - 1],
        };
        let end = floor_char_boundary(first, max_bytes);
        return Cut {
            text: format!("{}…", &first[..end]),
            total_lines,
            kept_lines: 0,
        };
    }
    if keep == Keep::Tail {
        kept.reverse();
    }
    Cut {
        text: kept.join("\n"),
        total_lines,
        kept_lines: kept.len(),
    }
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    (0..=max.min(s.len()))
        .rev()
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_keep_whole_lines_within_both_budgets() {
        let text = "a\nb\nc\nd";
        let head = cut(text, Keep::Head, 2, 100);
        assert_eq!((head.text.as_str(), head.truncated()), ("a\nb", true));
        let tail = cut(text, Keep::Tail, 2, 100);
        assert_eq!(tail.text, "c\nd");
        assert_eq!(cut(text, Keep::Head, 10, 4).text, "a\nb");
        assert!(!cut(text, Keep::Head, 10, 100).truncated());
    }

    #[test]
    fn an_oversized_line_shows_its_start() {
        let out = cut("ééééé", Keep::Head, 10, 5);
        assert_eq!(out.text, "éé…");
    }
}
