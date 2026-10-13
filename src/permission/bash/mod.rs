//! What a shell command line asks permission for: each command it runs, unless a pipe filter
//! only reads what another command writes or a reader only reads the files it names, plus its own
//! permissions for what cannot be read off the text (substitutions) and for writing files
//! (redirects).

pub mod filter;
pub mod split;
mod workdir;

use std::path::Path;

use crate::permission::catalog;
use crate::permission::gate::Ask;

/// `read` gives what reading one named file asks for, or `None` when it cannot be resolved.
pub fn asks(command: &str, project: &Path, read: &dyn Fn(&str) -> Option<Vec<Ask>>) -> Vec<Ask> {
    let split = split::split(command);
    // A reader (`grep -n x src/a.rs`) asks as the read tool would of each file it names, so the
    // read rules, secrets included, hold for it; past a `cd` its files are elsewhere.
    let mut reads = Vec::new();
    let mut commands = Vec::new();
    let mut moved = false;
    for command in &split.commands {
        let Some(command) = workdir::strip(command, project) else {
            continue;
        };
        moved |= workdir::moves(&command);
        let read = filter::reads_files(&command)
            .filter(|_| !moved)
            .and_then(|files| files.iter().map(|f| read(f)).collect::<Option<Vec<_>>>())
            .map(|asks| asks.concat());
        match read {
            Some(asks) => reads.extend(asks),
            None => commands.push(command),
        }
    }
    // A filter at the end of a pipe (`| tail -30`) reads only what the command before it
    // writes, so it asks for nothing of its own while something feeds it.
    let fed: Vec<String> = commands
        .iter()
        .filter(|c| !filter::reads_only_input(c))
        .cloned()
        .collect();
    let patterns = if fed.is_empty() && reads.is_empty() {
        commands
    } else {
        fed
    };
    let mut asks = reads;
    if !patterns.is_empty() {
        asks.push(Ask {
            permission: catalog::BASH.name.into(),
            patterns,
        });
    }
    // So read-only allowances never cover them.
    if split.complex {
        asks.push(Ask::new(catalog::BASH_COMPLEX.name, command.trim()));
    }
    if split.writes {
        asks.push(Ask::new(catalog::BASH_WRITE.name, command.trim()));
    }
    asks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asks(command: &str) -> Vec<Ask> {
        super::asks(command, Path::new("/nowhere"), &|f| {
            Some(vec![Ask::new(catalog::READ.name, f)])
        })
    }

    #[test]
    fn asks_once_per_command() {
        let asks = asks("git status && cargo test --all");
        assert_eq!(asks[0].patterns, vec!["git status", "cargo test --all"]);
        assert_eq!(asks.len(), 1);
    }

    #[test]
    fn a_pipe_filter_asks_nothing_while_something_feeds_it() {
        let patterns = |command: &str| asks(command).remove(0).patterns;
        assert_eq!(
            patterns("cargo test 2>&1 | grep -E 'test result|FAILED' | tail -5"),
            ["cargo test 2>&1"]
        );
        assert_eq!(
            asks("cargo test | tail .env"),
            [Ask::new("read", ".env"), Ask::new("bash", "cargo test")]
        );
        assert_eq!(patterns("tail -5"), ["tail -5"]);
    }

    #[test]
    fn a_file_that_cannot_be_resolved_leaves_its_reader_an_ordinary_command() {
        let asks = super::asks("cat a b", Path::new("/nowhere"), &|f| {
            (f == "a").then(|| vec![Ask::new("read", f)])
        });
        assert_eq!(asks, [Ask::new("bash", "cat a b")]);
    }

    #[test]
    fn substitutions_and_redirects_ask_separately() {
        let perms: Vec<String> = asks("echo $(date) > out.txt")
            .into_iter()
            .map(|a| a.permission)
            .collect();
        assert_eq!(perms, vec!["bash", "bash_complex", "bash_write"]);
    }
}
