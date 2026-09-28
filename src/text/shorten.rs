/// Shortens one line to `max` characters, marking the cut.
pub fn line(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(line("héllo", 2), "hé…");
        assert_eq!(line("hi", 5), "hi");
    }
}
