//! Commands: `crowbot <name>` on the command line, `/name` inside a session. One file per
//! command, one line in `COMMANDS`; where each works is data (`scope` in its spec).

mod help;
mod keytest;
pub mod login;
mod logout;
mod mode;
pub mod models;
mod quit;
mod signup;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use crate::app::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Cli,
    Session,
}

/// A command's user-facing metadata, from `data/commands/<name>.toml`. A command may keep
/// its own extra data in the same file, so unknown fields are allowed here.
#[derive(Debug, Deserialize)]
pub struct Spec {
    pub name: String,
    pub summary: String,
    pub usage: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default = "everywhere")]
    pub scope: Vec<Scope>,
}

fn everywhere() -> Vec<Scope> {
    vec![Scope::Cli, Scope::Session]
}

impl Spec {
    pub fn parse(src: &str) -> Self {
        toml::from_str(src).expect("command specs are checked by tests")
    }
}

pub struct Ctx<'a> {
    pub app: &'a App,
    pub scope: Scope,
}

/// What a command asks the session to do beyond showing its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Quit,
    /// Switch to the next mode, as Shift+Tab does.
    CycleMode,
    SetMode(String),
    /// Open the session's login card.
    Login,
    /// Open the session's model picker, after fetching a fresh list when asked.
    PickModel {
        refresh: bool,
    },
}

/// Markdown to show, and effects for the session to apply.
#[derive(Debug, Default)]
pub struct Outcome {
    pub text: String,
    pub effects: Vec<Effect>,
}

impl From<String> for Outcome {
    fn from(text: String) -> Self {
        Self {
            text,
            effects: Vec::new(),
        }
    }
}

impl From<&str> for Outcome {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

pub trait Command: Sync {
    fn spec(&self) -> &Spec;
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>>;
}

pub static COMMANDS: &[&dyn Command] = &[
    &help::Help,
    &login::Login,
    &logout::Logout,
    &signup::Signup,
    &models::Models,
    &mode::Mode,
    &quit::Quit,
    &keytest::KeyTest,
];

/// A command by name, if it exists at all (whatever its scope).
pub fn find(name: &str) -> Option<&'static dyn Command> {
    COMMANDS.iter().copied().find(|c| c.spec().name == name)
}

pub fn available(scope: Scope) -> impl Iterator<Item = &'static dyn Command> {
    COMMANDS
        .iter()
        .copied()
        .filter(move |c| c.spec().scope.contains(&scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_with_unique_names_and_a_scope() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.spec().name.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate command names");
        for command in COMMANDS {
            let spec = command.spec();
            assert!(!spec.summary.is_empty(), "{} has no summary", spec.name);
            assert!(!spec.scope.is_empty(), "{} works nowhere", spec.name);
            assert!(
                spec.usage.starts_with(&spec.name),
                "{} usage must start with its name",
                spec.name
            );
        }
    }
}
