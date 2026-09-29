use std::sync::LazyLock;

use anyhow::bail;
use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::auth;
use crate::settings;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/logout.toml")));

pub struct Logout;

impl Command for Logout {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if !args.is_empty() {
                bail!("usage: {}", SPEC.usage);
            }
            let removed = auth::remove(&cx.app.paths)?;
            // A running session stops using the key at once, unless the environment supplies one.
            let left = auth::load(&cx.app.paths)?.map(|key| key.secret);
            cx.app.api.set_key(left);
            let mut out = if removed {
                // crowbot refuses to revoke the key making the request, so it can only go elsewhere.
                "Logged out. The key still works until revoked with `crowbot keys` on another machine or on chat.crowbot.sh.".to_owned()
            } else {
                "No key was stored on this machine.".to_owned()
            };
            if settings::env("CROWBOT_API_KEY").is_some() {
                out.push_str(" CROWBOT_API_KEY is still set in this environment.");
            }
            Ok(out.into())
        })
    }
}
