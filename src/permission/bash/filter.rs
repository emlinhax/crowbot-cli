//! Pipe filters: a command that, as written, reads only its standard input and writes only its
//! output (`tail -30`, `grep -c ok`), or else only the files it names (`grep -n ok src/a.rs`).
//! Which commands and flags qualify is data/filters.toml.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static FILTERS: LazyLock<Vec<Filter>> = data::load!("filters.toml", "filter");

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
    /// Plain words past the positional ones are files it only reads.
    #[serde(default)]
    files: bool,
}

/// Whether `command` (one command of a split line) reads nothing but what is piped into it.
pub fn reads_only_input(command: &str) -> bool {
    parse(command).is_some_and(|(_, plain, allowed)| plain.len() <= allowed)
}

/// The files `command` names when, as written, it reads only those and writes only its output.
pub fn reads_files(command: &str) -> Option<Vec<String>> {
    let (filter, mut plain, allowed) = parse(command)?;
    (filter.files && plain.len() > allowed).then(|| plain.split_off(allowed))
}

/// The filter, its plain words, and how many of those are arguments rather than files.
fn parse(command: &str) -> Option<(&'static Filter, Vec<String>, usize)> {
    let words = words(command)?;
    let (name, args) = words.split_first()?;
    let filter = FILTERS.iter().find(|f| &f.name == name)?;
    let (plain, allowed) = filter.operands(args)?;
    Some((filter, plain, allowed))
}

impl Filter {
    fn operands(&self, args: &[String]) -> Option<(Vec<String>, usize)> {
        let has = |list: &[String], flag: &str| list.iter().any(|f| f == flag);
        let mut plain = Vec::new();
        let mut replaced = false;
        let mut only_plain = false;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if only_plain || arg == "-" || !arg.starts_with('-') {
                if arg != "-" {
                    plain.push(arg.clone());
                }
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
                        return None;
                    }
                } else if !has(&self.flags, &name) {
                    return None;
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
                        return None;
                    }
                    break;
                }
                if !has(&self.flags, &flag) {
                    return None;
                }
            }
        }
        let allowed = if replaced {
            self.positional.saturating_sub(1)
        } else {
            self.positional
        };
        Some((plain, allowed))
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
    fn a_reader_names_only_the_files_it_reads() {
        let files = |command: &str| reads_files(command);
        assert_eq!(files("grep -n foo src/a.py"), Some(vec!["src/a.py".into()]));
        assert_eq!(files("grep -e foo a b"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(files("head -30 more.pyi"), Some(vec!["more.pyi".into()]));
        assert_eq!(files("cat -n 'my file'"), Some(vec!["my file".into()]));
        assert_eq!(
            files("jq .name package.json"),
            Some(vec!["package.json".into()])
        );
        for not_a_reader in [
            "grep foo",
            "tail -5",
            "grep -r foo .",
            "grep -f patterns.txt x",
            "tail -f log.txt",
            "uniq a b",
            "tr a b",
            "sort -o out.txt in.txt",
            "cat *.py",
            "cat $FILE",
            "cat a > b",
            "sed -n 1p a",
            "awk 'NR>100' a",
        ] {
            assert_eq!(files(not_a_reader), None, "{not_a_reader}");
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
