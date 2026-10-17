//! JSON with comments and trailing commas (JSONC), as editors write their settings: made plain
//! JSON, so serde reads it.

/// `text` without `//` and `/* */` comments and without a comma before a closing `}` or `]`,
/// leaving strings as they are.
pub fn plain(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = comment_end(&chars, i) {
            i = end;
            continue;
        }
        match chars[i] {
            '"' => {
                let end = string_end(&chars, i);
                out.extend(&chars[i..end]);
                i = end;
            }
            ',' if closes_next(&chars, i + 1) => i += 1,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Just past the comment that opens at `i`, if one does.
fn comment_end(chars: &[char], i: usize) -> Option<usize> {
    match (chars.get(i), chars.get(i + 1)) {
        (Some('/'), Some('/')) => Some(
            chars[i..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |n| i + n),
        ),
        (Some('/'), Some('*')) => Some(
            (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '*' && chars[j + 1] == '/')
                .map_or(chars.len(), |j| j + 2),
        ),
        _ => None,
    }
}

/// Just past the string that opens at `start`, escapes included.
fn string_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Whether the next thing after `from`, past whitespace and comments, closes an object or array.
fn closes_next(chars: &[char], from: usize) -> bool {
    let mut i = from;
    while i < chars.len() {
        if let Some(end) = comment_end(chars, i) {
            i = end;
            continue;
        }
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            c => return c == '}' || c == ']',
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_go_and_strings_stay() {
        let text = r#"{
  // a line comment
  "servers": {
    "a": { "command": "x", "args": ["--url=http://h/*not*/", "//kept"], }, /* block */
  },
}"#;
        let value: serde_json::Value = serde_json::from_str(&plain(text)).unwrap();
        assert_eq!(value["servers"]["a"]["args"][0], "--url=http://h/*not*/");
        assert_eq!(value["servers"]["a"]["args"][1], "//kept");
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        let text = r#"{"a": "say \"hi\" // still text", }"#;
        let value: serde_json::Value = serde_json::from_str(&plain(text)).unwrap();
        assert_eq!(value["a"], "say \"hi\" // still text");
    }
}
