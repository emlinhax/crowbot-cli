//! What a command declares about itself: its names, usage and where it works (data/commands/),
//! and how it is typed in each place.

use serde::Deserialize;

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

/// A `LazyLock` of command `name`'s spec, from data/commands/<name>.toml, with `{key}` in its
/// usage filled in from each `key => value` given.
macro_rules! spec {
    ($name:literal $(, $key:literal => $value:expr)* $(,)?) => {
        std::sync::LazyLock::new(|| {
            let spec: $crate::commands::Spec = $crate::data::parse(
                $crate::data::raw!("commands/", $name, ".toml"),
                concat!("commands/", $name, ".toml"),
            );
            spec.fill(&[$(($key, &$value)),*])
        })
    };
}
pub(crate) use spec;

/// A command's user-facing metadata, from `data/commands/<name>.toml`. A command may keep
/// its own extra data in the same file, so unknown fields are allowed here.
#[derive(Debug, Deserialize)]
pub struct Spec {
    pub name: String,
    /// Other names it answers to (`/clear` for `/new`).
    #[serde(default)]
    pub aliases: Vec<String>,
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

impl Spec {
    /// The usage with `{key}` placeholders filled in, for usages that list a catalog's ids.
    pub fn fill(mut self, vars: &[(&str, &str)]) -> Self {
        self.usage = crate::text::template::fill(&self.usage, vars);
        self
    }
}

fn everywhere() -> Vec<Scope> {
    vec![Scope::Cli, Scope::Session]
}

impl Spec {
    /// Its name and its aliases.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.iter().map(String::as_str))
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

/// Where a command works, as it is typed there: `` `crowbot signup` ``.
pub fn places(spec: &Spec) -> String {
    spec.scope
        .iter()
        .map(|s| format!("`{}`", s.invoke(&spec.name)))
        .collect::<Vec<_>>()
        .join(" or ")
}
