use std::borrow::Cow;

/// Replaces each `{key}` that `value` knows, in one pass left to right: a filled-in value is
/// never scanned again, so a value holding `{other}` stays as it is. Unknown keys, and braces
/// that open no key, stay verbatim.
pub fn fill_with<'a>(
    template: &str,
    mut value: impl FnMut(&str) -> Option<Cow<'a, str>>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(['{', '}']) {
            Some(close) if after.as_bytes()[close] == b'}' => {
                let key = &after[..close];
                match value(key) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            // Another `{` first, or no `}` at all: this one opens nothing.
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `fill_with` over name-value pairs.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    fill_with(template, |key| {
        values
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, v)| Cow::Borrowed(*v))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_known_and_keeps_unknown() {
        assert_eq!(fill("{a} and {b}", &[("a", "x")]), "x and {b}");
    }

    #[test]
    fn a_value_is_never_filled_again() {
        let out = fill(
            "{path} at {date}",
            &[("path", "/tmp/{date}"), ("date", "today")],
        );
        assert_eq!(out, "/tmp/{date} at today");
    }

    #[test]
    fn braces_that_open_no_key_stay() {
        assert_eq!(fill("{ \"a\": 1 } {x", &[("x", "y")]), "{ \"a\": 1 } {x");
        assert_eq!(fill("{{x}}", &[("x", "y")]), "{y}");
    }
}
