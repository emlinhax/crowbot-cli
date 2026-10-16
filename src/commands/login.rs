use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec, account};
use crate::api::pair;
use crate::app::App;
use crate::auth::{self, KeyKind, LoggedIn, Origin};
use crate::data;
use crate::io::term;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("login");
static TEXT: LazyLock<Text> = data::load!("commands/login.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    pair: String,
    logged_in: String,
    status: String,
    from_env: String,
    env_note: String,
    logged_out: String,
    account: String,
    device: String,
}

pub struct Login;

impl Command for Login {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let number = match args {
                [flag] if flag == "--status" => return Ok(status(cx)?.into()),
                [] => None,
                [flag, number @ ..] if flag == "--key" && !number.is_empty() => {
                    Some(number.join(" "))
                }
                _ => return Err(cx.usage(&SPEC)),
            };
            // A session waits in its own card, which runs beside input.
            if cx.scope == Scope::Session {
                return Ok(Effect::Login { number }.into());
            }
            let text = match number {
                None => pair_here(cx.app).await?,
                Some(number) => logged_in(
                    &auth::adopt(&cx.app.api, &cx.app.paths, &number, KeyKind::Account).await?,
                ),
            };
            Ok(text.into())
        })
    }
}

async fn pair_here(app: &App) -> anyhow::Result<String> {
    let started = pair::start(&app.api).await?;
    let url = pair::page();
    let code = started.user_code.as_str();
    term::out(&format!(
        "{}\n",
        fill(&TEXT.pair, &[("url", &url), ("code", code)])
    ));
    Ok(logged_in(
        &auth::pair(&app.api, &app.paths, &started).await?,
    ))
}

/// What a successful login says, on the command line and in a session alike.
pub fn logged_in(done: &LoggedIn) -> String {
    let mut out = fill(
        &TEXT.logged_in,
        &[
            ("what", what(done.kind)),
            ("hint", &done.hint),
            ("standing", &account::standing(&done.me)),
        ],
    );
    out.push_str(env_note());
    out
}

/// Said after login, logout and signup while the environment's key shadows the stored one.
pub fn env_note() -> &'static str {
    if auth::env_shadows() {
        &TEXT.env_note
    } else {
        ""
    }
}

fn what(kind: KeyKind) -> &'static str {
    match kind {
        KeyKind::Account => &TEXT.account,
        KeyKind::Device => &TEXT.device,
    }
}

/// Which key this machine uses, or how to get one.
pub fn status(cx: &Ctx<'_>) -> anyhow::Result<String> {
    Ok(match auth::load(&cx.app.paths)? {
        None => fill(&TEXT.logged_out, &[("login", &cx.scope.invoke("login"))]),
        Some(key) => match key.origin {
            Origin::Env => TEXT.from_env.clone(),
            Origin::File { kind, hint } => {
                fill(&TEXT.status, &[("what", what(kind)), ("hint", &hint)])
            }
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::account::Me;

    #[test]
    fn text_parses_and_names_every_placeholder() {
        let me = Me {
            balance_microdollars: 12_300_000,
            can_spend: false,
            ..Me::default()
        };
        let done = LoggedIn {
            kind: KeyKind::Device,
            hint: auth::hint("device-key-9999"),
            me,
        };
        let out = logged_in(&done);
        assert!(out.starts_with("Logged in with device key …9999. Balance $12.30."));
        assert!(out.contains("cannot spend"));
        assert!(!out.contains('{'), "{out}");
    }
}
