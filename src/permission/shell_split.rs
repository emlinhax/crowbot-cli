//! Splits a shell command line into the commands it runs, so each can be matched against rules.
//! CEILING: a tokenizer, not a shell parser; substitutions and heredocs are flagged `complex`
//! and always go to the user. The upgrade is tree-sitter-bash, as opencode does.

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub commands: Vec<String>,
    /// `$(…)`, backticks, `<(…)`/`>(…)`, a subshell or group, or a heredoc: what runs cannot be
    /// read off the text.
    pub complex: bool,
    /// Output is redirected into a file.
    pub writes: bool,
}

pub fn split(line: &str) -> Split {
    let mut out = Split::default();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') => {
                quote = None;
                current.push(c);
            }
            (Some('\''), _) => current.push(c),
            (_, '\\') => {
                current.push(c);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some('"'), '"') => {
                quote = None;
                current.push(c);
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (_, '`') => {
                out.complex = true;
                current.push(c);
            }
            (_, '$') if chars.peek() == Some(&'(') => {
                out.complex = true;
                current.push(c);
            }
            (Some(_), _) => current.push(c),
            (None, ';' | '\n' | '|' | '&') => {
                // `||` and `&&` separate like `|` and `&`; `>&`/`&>` are redirects, not separators.
                if c == '&' && (current.ends_with('>') || chars.peek() == Some(&'>')) {
                    current.push(c);
                    continue;
                }
                if matches!(chars.peek(), Some(&n) if n == c) {
                    chars.next();
                }
                push(&mut out.commands, &mut current);
            }
            (None, '<') if chars.peek() == Some(&'<') => {
                out.complex = true;
                current.push(c);
            }
            // Process substitution runs its own command wherever it appears.
            (None, '<' | '>') if chars.peek() == Some(&'(') => {
                out.complex = true;
                current.push(c);
            }
            // A command that opens a subshell or group would be matched by its bracket, not by
            // the commands inside.
            (None, '(' | '{') if current.trim().is_empty() => {
                out.complex = true;
                current.push(c);
            }
            (None, '>') => {
                // `2>&1` and `>/dev/null` go nowhere worth asking about.
                let rest: String = chars.clone().collect();
                let target = rest.trim_start_matches(['>', '&', ' ']);
                if !(rest.starts_with('&')
                    || target.starts_with("/dev/null")
                    || target.starts_with("NUL"))
                {
                    out.writes = true;
                }
                current.push(c);
            }
            (None, _) => current.push(c),
        }
    }
    push(&mut out.commands, &mut current);
    out
}

fn push(commands: &mut Vec<String>, current: &mut String) {
    let command = current.trim();
    if !command.is_empty() {
        commands.push(command.to_owned());
    }
    current.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_separators_outside_quotes() {
        let s = split("cd src && cargo test | tee 'a|b' ; echo \"x && y\"");
        assert_eq!(
            s.commands,
            vec!["cd src", "cargo test", "tee 'a|b'", "echo \"x && y\""]
        );
        assert!(!s.complex && !s.writes);
    }

    #[test]
    fn flags_substitution_and_heredocs() {
        assert!(split("echo $(rm -rf /)").complex);
        assert!(split("echo `whoami`").complex);
        assert!(split("cat <<EOF\nx\nEOF").complex);
        assert!(!split("echo '$(literal)'").complex);
    }

    #[test]
    fn flags_process_substitution_subshells_and_groups() {
        assert!(split("cat <(rm -rf ~)").complex);
        assert!(split("diff <(ls a) <(ls b)").complex);
        assert!(split("tee >(sh)").complex);
        assert!(split("(rm -rf x)").complex);
        assert!(split("echo hi; (rm x)").complex);
        assert!(split("{ rm x; }").complex);
        assert!(!split("echo '<(x)' \"(y)\" a=(1 2)").complex);
    }

    #[test]
    fn flags_writes_but_not_stream_plumbing() {
        assert!(split("echo hi > out.txt").writes);
        assert!(split("echo hi >> out.txt").writes);
        assert!(!split("cargo build 2>&1").writes);
        assert!(!split("git status > /dev/null").writes);
        assert!(!split("echo '>' ").writes);
        assert_eq!(
            split("make 2>&1 | tail").commands,
            vec!["make 2>&1", "tail"]
        );
    }
}
