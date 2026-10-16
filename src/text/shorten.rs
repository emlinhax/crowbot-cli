//! Plain strings cut short, the cut marked.

/// What marks a cut.
pub const MARK: &str = "…";

/// Shortens one line to `max` characters.
pub fn line(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}{MARK}", &text[..i]),
        None => text.to_owned(),
    }
}

/// Shortens `text` to at most `max` bytes, on a character boundary.
pub fn bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!("{}{MARK}", &text[..text.floor_char_boundary(max)])
}

/// The last `max` characters of `text`, the cut marked in front.
pub fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    match text.char_indices().nth(count.saturating_sub(max)) {
        Some((i, _)) if count > max => format!("{MARK}{}", &text[i..]),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(line("héllo", 2), "hé…");
        assert_eq!(line("hi", 5), "hi");
        assert_eq!(tail("1234 5678", 4), "…5678");
        assert_eq!(tail("ab", 4), "ab");
    }

    #[test]
    fn a_byte_cut_never_splits_a_character() {
        assert_eq!(bytes("héllo", 2), "h…");
        assert_eq!(bytes("hello", 9), "hello");
    }
}
