use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;

use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::api::endpoints;
use crate::auth;
use crate::data;
use crate::io::term;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("signup");
static TEXT: LazyLock<Text> = data::load!("commands/signup.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    logged_in: String,
    creating: String,
    number: String,
    about: String,
    next: String,
    confirm: String,
    mismatch: String,
}

pub struct Signup;

impl Command for Signup {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let app = cx.app;
            let force = match args {
                [] => false,
                [flag] if flag == "--force" => true,
                _ => return Err(cx.usage(&SPEC)),
            };
            // A session asks in its own card, which runs beside input.
            if cx.scope == Scope::Session {
                return Ok(Effect::Signup { force }.into());
            }
            // A new account would replace the stored key, and an account number has no recovery.
            if !force && auth::stored(&app.paths)?.is_some() {
                let logout = cx.scope.invoke("logout");
                bail!(fill(&TEXT.logged_in, &[("logout", &logout)]));
            }
            term::out(&format!("{}\n", TEXT.creating));
            // Saved before it is shown, so an interrupted terminal cannot lose the only copy.
            let account = auth::sign_up(&app.api, &app.paths).await?;
            let path = app.paths.auth().display().to_string();
            term::out(&fill(
                &TEXT.number,
                &[("number", &account.formatted), ("about", &about(&path))],
            ));
            confirm(&account.number)?;
            Ok(next().into())
        })
    }
}

/// Asks for the number's tail back when stdin is a terminal.
fn confirm(number: &str) -> anyhow::Result<()> {
    if !term::stdin_is_terminal() {
        return Ok(());
    }
    loop {
        term::out(&confirm_prompt());
        let Some(line) = term::read_line()? else {
            return Ok(());
        };
        if confirms(number, &line) {
            return Ok(());
        }
        term::out(&format!("{}\n", TEXT.mismatch));
    }
}

/// What creating an account says while it works.
pub fn creating() -> &'static str {
    &TEXT.creating
}

/// What the number is, and where its copy is kept.
pub fn about(path: &str) -> String {
    fill(&TEXT.about, &[("path", path)])
}

/// The request for the number's last digits.
pub fn confirm_prompt() -> String {
    fill(&TEXT.confirm, &[("n", &auth::HINT_CHARS.to_string())])
}

pub fn mismatch() -> &'static str {
    &TEXT.mismatch
}

/// What to do with the new account.
pub fn next() -> String {
    let mut out = fill(&TEXT.next, &[("chat", &endpoints::origin("chat"))]);
    out.push_str(super::login::env_note());
    out
}

/// Whether `typed` is the number's last digits, the proof it was saved somewhere.
pub fn confirms(number: &str, typed: &str) -> bool {
    let digits: Vec<char> = number.chars().filter(char::is_ascii_digit).collect();
    let tail = &digits[digits.len().saturating_sub(auth::HINT_CHARS)..];
    typed.trim().chars().eq(tail.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_digits_confirm() {
        assert!(confirms("1234567890123456", " 3456\n"));
        assert!(confirms("1234 5678 9012 3456", "3456"));
        assert!(!confirms("1234567890123456", "456"));
        assert!(!confirms("1234567890123456", "1234"));
    }
}
