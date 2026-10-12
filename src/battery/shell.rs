//! The bash calls: which commands the model writes and with which flags, which are pipe filters,
//! and which stand in for a tool (the smells in data/commands/battery.toml).

use serde::Deserialize;

use super::Counts;
use super::load::Run;
use crate::permission::{filter, shell_split};

/// A shell command that does what a tool does. It matches a command that starts with `words`,
/// or whose call contains `contains`; the first smell that matches wins.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Smell {
    pub label: String,
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub contains: Option<String>,
    /// The tool, or the way, that would do instead.
    pub instead: String,
}

#[derive(Default)]
pub struct Shell {
    pub calls: usize,
    /// Calls whose commands cannot be read off their text (substitutions, heredocs).
    pub complex: usize,
    pub first: Counts,
    /// A subcommand program with its subcommand, `cargo test`.
    pub first_two: Counts,
    /// `grep -n`, `cargo test --quiet`: each flag with the command it was given to.
    pub flags: Counts,
    /// Commands that only read what is piped in, by name.
    pub filters: Counts,
    /// By smell label.
    pub smells: Counts,
}

pub fn tally(runs: &[Run], smells: &[Smell], subcommands: &[String]) -> Shell {
    let mut out = Shell::default();
    for call in runs
        .iter()
        .flat_map(|r| &r.calls)
        .filter(|c| c.name == "bash")
    {
        let Some(line) = call.args["command"].as_str() else {
            continue;
        };
        out.calls += 1;
        let split = shell_split::split(line);
        if split.complex {
            out.complex += 1;
        }
        for command in &split.commands {
            let words = words(command);
            let Some(first) = words.first() else {
                continue;
            };
            if filter::reads_only_input(command) {
                out.filters.add(*first, command);
                continue;
            }
            out.first.add(*first, command);
            let lead = lead(command, subcommands);
            for flag in words.iter().filter(|w| w.starts_with('-') && w.len() > 1) {
                let flag = flag.split('=').next().unwrap_or(flag);
                out.flags.add(format!("{lead} {flag}"), command);
            }
            out.first_two.add(lead, command);
            if let Some(smell) = smells.iter().find(|s| matches(s, &words, line)) {
                out.smells.add(&smell.label, command);
            }
        }
    }
    out
}

/// The command's words, without leading `NAME=value` assignments.
pub fn words(command: &str) -> Vec<&str> {
    command
        .split_whitespace()
        .skip_while(|w| {
            w.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
        .collect()
}

/// What a command is known by: its first word, with the subcommand when the program has them.
pub fn lead(command: &str, subcommands: &[String]) -> String {
    let words = words(command);
    match words.as_slice() {
        [first, second, ..]
            if subcommands.iter().any(|s| s == first) && !second.starts_with('-') =>
        {
            format!("{first} {second}")
        }
        [first, ..] => (*first).to_owned(),
        [] => String::new(),
    }
}

fn matches(smell: &Smell, words: &[&str], line: &str) -> bool {
    let by_words = !smell.words.is_empty()
        && smell.words.len() <= words.len()
        && smell.words.iter().zip(words).all(|(want, got)| want == got);
    let by_text = smell
        .contains
        .as_ref()
        .is_some_and(|c| line.contains(c.as_str()));
    by_words || by_text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;

    #[test]
    fn commands_filters_flags_and_stand_ins_are_counted() {
        let words = fixture::words();
        let shell = tally(&fixture::battery().runs, &words.smell, &words.subcommands);
        assert_eq!((shell.calls, shell.complex), (4, 0));
        assert_eq!((shell.first.get("cargo"), shell.first.get("sed")), (2, 1));
        assert_eq!(shell.first_two.get("cargo test"), 2);
        assert_eq!(shell.flags.get("python3 -m"), 1);
        assert_eq!(
            shell.flags.get("cargo test 2>&1"),
            0,
            "a redirect is not a flag"
        );
        assert_eq!(shell.filters.get("tail"), 2);
        assert_eq!(shell.flags.get("sed -n"), 1);
        assert_eq!(shell.smells.get("sed -n"), 1);
        assert_eq!(shell.smells.total(), 1);
    }

    #[test]
    fn a_command_is_known_by_its_program_and_subcommand() {
        let subs = vec!["git".to_owned()];
        assert_eq!(lead("RUST_LOG=1 git status -s", &subs), "git status");
        assert_eq!(lead("git -C x log", &subs), "git");
        assert_eq!(lead("ls src", &subs), "ls");
    }
}
