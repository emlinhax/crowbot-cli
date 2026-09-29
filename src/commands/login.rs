use std::fmt::Write;
use std::sync::LazyLock;

use anyhow::{Context, bail};
use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::api::account;
use crate::api::endpoints;
use crate::api::pair::{self, Poll};
use crate::app::App;
use crate::auth::{self, KeyKind, Origin};
use crate::io::{self, term};
use crate::text::units;
use crate::{limits, settings};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/login.toml")));

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
            let app = cx.app;
            let text = match args {
                [] => pair(app).await,
                [flag, number @ ..] if flag == "--key" && !number.is_empty() => {
                    with_account_number(app, &number.join(" ")).await
                }
                [flag] if flag == "--status" => status(app),
                _ => bail!("usage: {}", SPEC.usage),
            }?;
            Ok(text.into())
        })
    }
}

async fn pair(app: &App) -> anyhow::Result<String> {
    let started = pair::start(&app.api, &io::proc::hostname()).await?;
    let url = endpoints::url(endpoints::get("pair_page"), &[]);
    term::out(&format!(
        "Pair this machine with your crowbot account:\n  1. Open {url}\n  2. Sign in and enter the code  {}\n\nWaiting for approval (Ctrl+C to cancel)…\n",
        started.user_code
    ));
    // Tests and headless machines set this; the URL above is always printed anyway.
    if settings::env("CROWBOT_NO_BROWSER").is_none() {
        let _ = io::proc::open_url(&url);
    }

    let limits = &limits::get().pair;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(started.ttl);
    loop {
        match pair::poll(&app.api, &started.device_code).await {
            Ok(Poll::Ready(key)) => {
                auth::save(&app.paths, &key, KeyKind::Device)?;
                return Ok(format!(
                    "Paired. Device key {} saved to {}.",
                    auth::hint(&key),
                    app.paths.auth().display()
                ));
            }
            Ok(Poll::Pending) => tokio::time::sleep(limits.poll_interval_ms.ms()).await,
            Err(e) if e.info.kind == "rate_limited" => {
                tokio::time::sleep(limits.backoff_ms.ms()).await;
            }
            Err(e) => return Err(e.into()),
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("the pairing code expired; run `crowbot login` again");
        }
    }
}

async fn with_account_number(app: &App, number: &str) -> anyhow::Result<String> {
    let number = auth::normalize(number);
    let me = account::me(&app.api.with_key(number.clone()))
        .await
        .context("checking that key with crowbot")?;
    auth::save(&app.paths, &number, KeyKind::Account)?;
    let mut out = format!(
        "Logged in with account {}. Balance {}.",
        auth::hint(&number),
        units::usd(me.balance_microdollars as f64 / 1e6)
    );
    if !me.can_spend {
        out.push_str(" The account cannot spend yet; top it up at chat.crowbot.sh.");
    }
    Ok(out)
}

fn status(app: &App) -> anyhow::Result<String> {
    let mut out = String::new();
    match auth::load(&app.paths)? {
        None => out.push_str("Not logged in. Run `crowbot login`."),
        Some(key) => match key.origin {
            Origin::Env => out.push_str("Using CROWBOT_API_KEY from the environment."),
            Origin::File { kind, hint } => {
                let what = match kind {
                    KeyKind::Account => "account number",
                    KeyKind::Device => "device key",
                };
                let _ = write!(out, "Logged in with {what} {hint}.");
            }
        },
    }
    Ok(out)
}
