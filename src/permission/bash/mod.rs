//! What a shell command line asks permission for: each command it runs, unless a pipe filter
//! only reads what another command writes, plus its own permissions for what cannot be read
//! off the text (substitutions) and for writing files (redirects).

mod filter;
mod split;

use crate::permission::catalog;
use crate::permission::gate::Ask;

pub fn asks(command: &str) -> Vec<Ask> {
    let split = split::split(command);
    // A filter at the end of a pipe (`| tail -30`) reads only what the command before it
    // writes, so it asks for nothing of its own while something feeds it.
    let fed: Vec<String> = split
        .commands
        .iter()
        .filter(|c| !filter::reads_only_input(c))
        .cloned()
        .collect();
    let patterns = if fed.is_empty() { split.commands } else { fed };
    let mut asks = vec![Ask {
        permission: catalog::BASH.name.into(),
        patterns,
    }];
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
            patterns("cargo test | tail .env"),
            ["cargo test", "tail .env"]
        );
        assert_eq!(patterns("tail -5"), ["tail -5"]);
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
