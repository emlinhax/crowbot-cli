//! Slash commands, each also reachable as `crowbot <name>`: one file per command, one line below.

mod help;
mod keytest;
mod login;
mod logout;
mod models;
mod signup;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use crate::app::App;

/// A command's user-facing metadata, from `data/commands/<name>.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    pub summary: String,
    pub usage: String,
    #[serde(default)]
    pub hidden: bool,
}

impl Spec {
    pub fn parse(src: &str) -> Self {
        toml::from_str(src).expect("command specs are checked by tests")
    }
}

pub trait Command: Sync {
    fn spec(&self) -> &Spec;
    /// Returns markdown for the caller to show; interactive commands drive the terminal themselves.
    fn run<'a>(&'a self, app: &'a App, args: &'a [String])
    -> BoxFuture<'a, anyhow::Result<String>>;
}

pub static COMMANDS: &[&dyn Command] = &[
    &help::Help,
    &login::Login,
    &logout::Logout,
    &signup::Signup,
    &models::Models,
    &keytest::KeyTest,
];

pub fn find(name: &str) -> Option<&'static dyn Command> {
    COMMANDS.iter().copied().find(|c| c.spec().name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_with_unique_names() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.spec().name.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate command names");
        for command in COMMANDS {
            let spec = command.spec();
            assert!(!spec.summary.is_empty(), "{} has no summary", spec.name);
            assert!(
                spec.usage.starts_with(&spec.name),
                "{} usage must start with its name",
                spec.name
            );
        }
    }
}
