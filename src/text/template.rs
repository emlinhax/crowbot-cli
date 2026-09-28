/// Replaces each `{name}` in `template`; unknown placeholders are left as they are.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_known_and_keeps_unknown() {
        assert_eq!(fill("{a} and {b}", &[("a", "x")]), "x and {b}");
    }
}
