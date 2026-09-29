use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::api::signup;
use crate::app::App;
use crate::auth::{self, KeyKind};
use crate::io::term;
use crate::limits;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/signup.toml")));

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
                _ => bail!("usage: {}", SPEC.usage),
            };
            // A new account would replace the stored key, and an account number has no recovery.
            if !force && auth::load(&app.paths)?.is_some() {
                bail!(
                    "this machine is already logged in; `crowbot logout` first, or pass --force if the current key is saved elsewhere"
                );
            }
            term::out("Creating a crowbot account (solving a proof-of-work challenge)…\n");
            let account = create(app).await?;
            // Saved before it is shown, so an interrupted terminal cannot lose the only copy.
            auth::save(&app.paths, &account.account_number, KeyKind::Account)?;
            term::out(&format!(
                "\nYour account number:  {}\n\nThis number is your login and your API key. There is no email and no recovery,\nso store it in a password manager now. A copy is saved in {}.\n",
                account.formatted,
                app.paths.auth().display()
            ));
            confirm(&account.account_number)?;
            Ok(Outcome::from("Next: fund it at https://chat.crowbot.sh."))
        })
    }
}

async fn create(app: &App) -> anyhow::Result<signup::Account> {
    let attempts = limits::get().signup.max_attempts.value;
    let mut last = None;
    for _ in 0..attempts {
        let challenge = signup::challenge(&app.api).await?;
        let seed = challenge.challenge.clone();
        let nonce =
            tokio::task::spawn_blocking(move || signup::solve(&seed, challenge.bits)).await?;
        match signup::create(&app.api, &challenge.challenge, &nonce).await {
            Ok(account) => return Ok(account),
            // The required work grew while we solved; fetch a fresh challenge.
            Err(e) if e.info.kind == "pow_required" => last = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(last.map_or_else(|| anyhow::anyhow!("signup failed"), Into::into))
}

/// Asks for the last four digits back, when someone is at the keyboard to answer.
fn confirm(number: &str) -> anyhow::Result<()> {
    if !term::stdin_is_terminal() {
        return Ok(());
    }
    let tail: String = number
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    loop {
        term::out("Type its last 4 digits to confirm you saved it: ");
        let Some(line) = term::read_line()? else {
            return Ok(());
        };
        if line.trim() == tail {
            return Ok(());
        }
        term::out("That does not match; check what you saved.\n");
    }
}
