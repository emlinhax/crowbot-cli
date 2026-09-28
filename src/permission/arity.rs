use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/command_arity.toml");

static ARITY: LazyLock<BTreeMap<String, usize>> = LazyLock::new(|| {
    toml::from_str::<Catalog>(SRC)
        .expect("data/command_arity.toml is checked by tests")
        .arity
});

#[derive(Deserialize)]
struct Catalog {
    arity: BTreeMap<String, usize>,
}

/// The rule pattern "always allow" should add for `command`, e.g. `git commit *`.
pub fn always_pattern(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().collect();
    let keep = ARITY
        .iter()
        .filter(|(prefix, _)| {
            let prefix: Vec<&str> = prefix.split_whitespace().collect();
            words.starts_with(&prefix)
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map_or(1, |(_, n)| *n);
    if words.len() <= keep {
        return words.join(" ");
    }
    format!("{} *", words[..keep].join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_identifying_words() {
        assert_eq!(always_pattern("git commit -m 'x y'"), "git commit *");
        assert_eq!(always_pattern("npm run build --watch"), "npm run build *");
        assert_eq!(always_pattern("ls -la src"), "ls *");
        assert_eq!(always_pattern("cargo test"), "cargo test");
    }
}
