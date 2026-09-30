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
use crate::auth;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Cli,
    Session,
}

impl Scope {
    /// How a command is typed here: `crowbot login`, `/login`.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Cli => "crowbot ",
            Self::Session => "/",
        }
    }

    pub fn invoke(self, rest: &str) -> String {
        format!("{}{rest}", self.prefix())
    }
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
    /// Flags whose values are secrets: echoed and remembered only as a hint.
    #[serde(default)]
    pub secret: Vec<String>,
}

fn everywhere() -> Vec<Scope> {
    vec![Scope::Cli, Scope::Session]
}

impl Spec {
    pub fn parse(src: &str) -> Self {
        toml::from_str(src).expect("command specs are checked by tests")
    }

    /// `args` as they may be shown and remembered: a secret flag's value becomes a hint.
    pub fn redact(&self, args: &[String]) -> String {
        let mut out = Vec::new();
        let mut hidden: Option<String> = None;
        for arg in args {
            if arg.starts_with("--") {
                out.extend(hidden.take().map(|h| auth::hint(&h)));
                match arg.split_once('=') {
                    Some((flag, value)) if self.secret.iter().any(|s| s == flag) => {
                        out.push(format!("{flag}={}", auth::hint(value)));
                    }
                    _ => {
                        out.push(arg.clone());
                        if self.secret.contains(arg) {
                            hidden = Some(String::new());
                        }
                    }
                }
            } else if let Some(h) = hidden.as_mut() {
                h.push_str(arg);
            } else {
                out.push(arg.clone());
            }
        }
        out.extend(hidden.filter(|h| !h.is_empty()).map(|h| auth::hint(&h)));
        out.join(" ")
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

pub enum Missing {
    Unknown,
    /// It exists, but not in the scope asked about.
    Elsewhere(&'static Spec),
}

/// The command `name` names in `scope`; one that only works elsewhere says so.
pub fn lookup(name: &str, scope: Scope) -> Result<&'static dyn Command, Missing> {
    let command = find(name).ok_or(Missing::Unknown)?;
    if command.spec().scope.contains(&scope) {
        Ok(command)
    } else {
        Err(Missing::Elsewhere(command.spec()))
    }
}

/// A typed command line, `name args…`, with its secret values replaced by hints.
pub fn redact_line(line: &str) -> String {
    let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    match words.split_first() {
        Some((name, args)) if !args.is_empty() => match find(name) {
            Some(command) => format!("{name} {}", command.spec().redact(args)),
            None => line.trim().to_owned(),
        },
        _ => line.trim().to_owned(),
    }
}

/// Where a command works, as it is typed there: `` `crowbot signup` ``.
pub fn places(spec: &Spec) -> String {
    spec.scope
        .iter()
        .map(|s| format!("`{}`", s.invoke(&spec.name)))
        .collect::<Vec<_>>()
        .join(" or ")
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
    fn a_command_from_another_scope_says_where_it_works() {
        assert!(lookup("help", Scope::Session).is_ok());
        assert!(matches!(
            lookup("nope", Scope::Session),
            Err(Missing::Unknown)
        ));
        let Err(Missing::Elsewhere(spec)) = lookup("signup", Scope::Session) else {
            panic!("signup works in the session");
        };
        assert_eq!(places(spec), "`crowbot signup`");
    }

    #[test]
    fn secret_values_are_shown_only_as_a_hint() {
        let words = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let login = find("login").unwrap().spec();
        assert_eq!(login.redact(&words("--key 1234 5678")), "--key …5678");
        assert_eq!(login.redact(&words("--key=12345678")), "--key=…5678");
        assert_eq!(login.redact(&words("--status")), "--status");
        assert_eq!(
            redact_line("login --key 1234 5678 9012 3456"),
            "login --key …3456"
        );
        assert_eq!(redact_line("models --refresh"), "models --refresh");
    }

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
