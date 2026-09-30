use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::api::account::Me;
use crate::api::pair;
use crate::app::App;
use crate::auth::{self, KeyKind, Origin};
use crate::io::{self, term};
use crate::settings;
use crate::text::template::fill;
use crate::text::units;

const SRC: &str = include_str!("../../data/commands/login.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/login.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    pair: String,
    logged_in: String,
    cannot_spend: String,
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
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::Login { number }],
                });
            }
            let text = match number {
                None => pair_here(cx.app).await?,
                Some(number) => {
                    let me = auth::adopt(cx.app, &number, KeyKind::Account).await?;
                    logged_in(KeyKind::Account, &number, &me)
                }
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
    open_browser(&url);
    let key = pair::wait(&app.api, &started).await?;
    let me = auth::adopt(app, &key, KeyKind::Device).await?;
    Ok(logged_in(KeyKind::Device, &key, &me))
}

/// Opens the pairing page unless `CROWBOT_NO_BROWSER` is set (tests, headless machines); the
/// URL is always shown too.
pub fn open_browser(url: &str) {
    if settings::env("CROWBOT_NO_BROWSER").is_none() {
        let _ = io::proc::open_url(url);
    }
}

/// What a successful login says, on the command line and in a session alike.
pub fn logged_in(kind: KeyKind, secret: &str, me: &Me) -> String {
    let balance = units::usd_micros(me.balance_microdollars);
    let mut out = fill(
        &TEXT.logged_in,
        &[
            ("what", what(kind)),
            ("hint", &auth::hint(secret)),
            ("balance", &balance),
        ],
    );
    if !me.can_spend {
        out.push_str(&TEXT.cannot_spend);
    }
    out.push_str(env_note());
    out
}

/// Said after login, logout and signup while the environment's key shadows the stored one.
pub fn env_note() -> &'static str {
    if auth::from_env().is_some() {
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

fn status(cx: &Ctx<'_>) -> anyhow::Result<String> {
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

    #[test]
    fn text_parses_and_names_every_placeholder() {
        let me = Me {
            balance_microdollars: 12_300_000,
            can_spend: false,
        };
        let out = logged_in(KeyKind::Device, "device-key-9999", &me);
        assert!(out.starts_with("Logged in with device key …9999. Balance $12.30."));
        assert!(out.contains("cannot spend"));
        assert!(!out.contains('{'), "{out}");
    }
}
