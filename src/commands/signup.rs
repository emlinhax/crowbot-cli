use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;

use serde::Deserialize;

use super::{Command, Ctx, Outcome, Spec};
use crate::api::{endpoints, signup};
use crate::auth::{self, KeyKind};
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
            // A new account would replace the stored key, and an account number has no recovery.
            if !force && auth::stored(&app.paths)?.is_some() {
                let logout = cx.scope.invoke("logout");
                bail!(fill(&TEXT.logged_in, &[("logout", &logout)]));
            }
            term::out(&format!("{}\n", TEXT.creating));
            let account = signup::register(&app.api).await?;
            // Saved before it is shown, so an interrupted terminal cannot lose the only copy.
            auth::save(&app.paths, &account.account_number, KeyKind::Account)?;
            let path = app.paths.auth().display().to_string();
            term::out(&fill(
                &TEXT.number,
                &[("number", &account.formatted), ("path", &path)],
            ));
            confirm(&account.account_number)?;
            let mut out = fill(&TEXT.next, &[("chat", &endpoints::origin("chat"))]);
            out.push_str(super::login::env_note());
            Ok(out.into())
        })
    }
}

/// Asks for the number's tail back when stdin is a terminal.
fn confirm(number: &str) -> anyhow::Result<()> {
    if !term::stdin_is_terminal() {
        return Ok(());
    }
    let tail: String = number
        .chars()
        .rev()
        .take(auth::HINT_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    loop {
        term::out(&fill(
            &TEXT.confirm,
            &[("n", &auth::HINT_CHARS.to_string())],
        ));
        let Some(line) = term::read_line()? else {
            return Ok(());
        };
        if line.trim() == tail {
            return Ok(());
        }
        term::out(&format!("{}\n", TEXT.mismatch));
    }
}
