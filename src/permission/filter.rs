//! Pipe filters: a command that, as written, reads only its standard input and writes only its
//! output (`tail -30`, `grep -c ok`). Which commands and flags qualify is data/filters.toml.

use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/filters.toml");

static FILTERS: LazyLock<Vec<Filter>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        filter: Vec<Filter>,
    }
    toml::from_str::<File>(SRC)
        .expect("data/filters.toml is checked by tests")
        .filter
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    name: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    valued: Vec<String>,
    #[serde(default)]
    numeric: bool,
    #[serde(default)]
    positional: usize,
    #[serde(default)]
    replaces_positional: Vec<String>,
}

/// Whether `command` (one command of a split line) reads nothing but what is piped into it.
pub fn reads_only_input(command: &str) -> bool {
    let Some(words) = words(command) else {
        return false;
    };
    let Some((name, args)) = words.split_first() else {
        return false;
    };
    FILTERS
        .iter()
        .find(|f| &f.name == name)
        .is_some_and(|f| f.accepts(args))
}

impl Filter {
    fn accepts(&self, args: &[String]) -> bool {
        let has = |list: &[String], flag: &str| list.iter().any(|f| f == flag);
        let mut plain = 0;
        let mut replaced = false;
        let mut only_plain = false;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if only_plain || arg == "-" || !arg.starts_with('-') {
                plain += usize::from(arg != "-");
                continue;
            }
            if arg == "--" {
                only_plain = true;
                continue;
            }
            if let Some(long) = arg.strip_prefix("--") {
                let name = format!("--{}", long.split('=').next().unwrap_or(long));
                if has(&self.valued, &name) {
                    replaced |= has(&self.replaces_positional, &name);
                    if !long.contains('=') && args.next().is_none() {
                        return false;
                    }
                } else if !has(&self.flags, &name) {
                    return false;
                }
                continue;
            }
            let bundle = &arg[1..];
            if self.numeric && bundle.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            for (i, c) in bundle.char_indices() {
                let flag = format!("-{c}");
                if has(&self.valued, &flag) {
                    replaced |= has(&self.replaces_positional, &flag);
                    // The value is the rest of the word, or else the next one.
                    if i + c.len_utf8() == bundle.len() && args.next().is_none() {
                        return false;
                    }
                    break;
                }
                if !has(&self.flags, &flag) {
                    return false;
                }
            }
        }
        let allowed = if replaced {
            self.positional.saturating_sub(1)
        } else {
            self.positional
        };
        plain <= allowed
    }
}

/// The command's words with quotes removed, or `None` when the shell would change them first: an
/// unquoted glob, `$`, `~`, brace or redirect could turn a pattern into a file name.
fn words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '$' | '`' | '\\') => return None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                quoted = true;
            }
            (None, '\\') => word.push(chars.next()?),
            (None, '*' | '?' | '[' | ']' | '{' | '}' | '$' | '~' | '<' | '>' | '`') => {
                return None;
            }
            (None, c) if c.is_whitespace() => {
                if quoted || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                quoted = false;
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if quoted || !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_parse() {
        assert!(FILTERS.iter().any(|f| f.name == "tail"));
    }

    #[test]
    fn a_filter_reading_only_its_input_passes() {
        for command in [
            "tail -30",
            "tail -n 30",
            "tail -n +3",
            "head -c5",
            "head --lines=20",
            "wc -l",
            "sort -rn",
            "sort -t: -k2",
            "uniq -c",
            "cut -d, -f2",
            "cut -d , -f 2",
            "tr -d '\\r'",
            "tr a-z A-Z",
            "grep -E \"test result|FAILED\"",
            "grep -c ok",
            "grep -in -A3 error",
            "grep -e foo -e bar",
            "grep -v -- -x",
            "jq -r '.name'",
        ] {
            assert!(reads_only_input(command), "{command}");
        }
    }

    #[test]
    fn anything_that_could_read_a_file_is_an_ordinary_command() {
        for command in [
            "tail .env",
            "tail -n 5 .env",
            "tail -f log.txt",
            "tail -5 < .env",
            "tail -n *",
            "head -5 $FILE",
            "head -5 ~/.ssh/id_rsa",
            "grep foo .env",
            "grep -e foo .env",
            "grep -r foo",
            "grep -rn foo",
            "grep -f patterns.txt",
            "grep \"$(cat .env)\"",
            "sort -o out.txt",
            "sort --files0-from=list",
            "uniq a b",
            "wc -l .env",
            "jq . data.json",
            "jq --rawfile x .env .",
            "sed -n 1p",
            "tail -30 2>&1",
            "tail -n",
            "/usr/bin/tail -5",
            "LC_ALL=C sort",
            "grep 'unterminated",
        ] {
            assert!(!reads_only_input(command), "{command}");
        }
    }
}
