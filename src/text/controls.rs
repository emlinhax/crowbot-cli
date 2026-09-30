//! Keeping other people's control characters off the terminal. Model replies, server errors and
//! command output can carry escape sequences (OSC 52 writes the clipboard, others retitle or
//! scramble the screen); only crowbot's own rendering may emit them.

use std::borrow::Cow;
use std::iter::Peekable;
use std::str::Chars;

/// `text` without escape sequences (CSI, OSC and two-character ones); everything else kept.
pub fn strip_escapes(text: &str) -> Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// One row's worth of text that may reach the terminal: no escapes, no control characters, and
/// line breaks as spaces.
pub fn for_terminal(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    strip_escapes(text)
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect::<String>()
        .into()
}

/// A URL safe inside an OSC 8 hyperlink: control characters and spaces percent-encoded.
pub fn osc_url(url: &str) -> Cow<'_, str> {
    if !url.chars().any(|c| c.is_control() || c == ' ') {
        return Cow::Borrowed(url);
    }
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        if c.is_control() || c == ' ' {
            let mut buf = [0; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Skips what follows an ESC: a CSI up to its final byte, an OSC up to BEL or ESC \, else one
/// character.
fn skip_escape(chars: &mut Peekable<Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for n in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&n) {
                    break;
                }
            }
        }
        Some(']' | 'P' | '_' | '^') => {
            while let Some(n) = chars.next() {
                if n == '\u{7}' || (n == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_never_pass() {
        let clipboard = "copy \u{1b}]52;c;cm0gLXJmIH4=\u{7}this";
        assert_eq!(for_terminal(clipboard), "copy this");
        assert_eq!(for_terminal("\u{1b}]0;title\u{1b}\\x"), "x");
        assert_eq!(for_terminal("\u{1b}[2J\u{1b}[31mred"), "red");
        assert_eq!(for_terminal("a\u{9b}31mb\u{7}c"), "a31mbc");
        assert_eq!(for_terminal("two\nlines"), "two lines");
        assert!(matches!(for_terminal("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn stripping_escapes_keeps_line_structure() {
        assert_eq!(strip_escapes("\u{1b}[1mA\u{1b}[0m\r\nB"), "A\r\nB");
    }

    #[test]
    fn link_urls_cannot_close_their_own_sequence() {
        assert_eq!(
            osc_url("https://x.dev/a b\u{1b}\\\u{7}"),
            "https://x.dev/a%20b%1B\\%07"
        );
    }
}
