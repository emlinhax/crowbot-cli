use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::auth;

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
            cx.no_args(&SPEC, args)?;
            let removed = auth::remove(&cx.app.paths)?;
            // A running session stops using the key at once, unless the environment supplies one.
            let left = auth::load(&cx.app.paths)?.map(|key| key.secret);
            cx.app.api.set_key(left);
            let mut out = if removed {
                // crowbot refuses to revoke the key making the request, so it can only go elsewhere.
                "Logged out. The key still works until revoked on chat.crowbot.sh.".to_owned()
            } else {
                "No key was stored on this machine.".to_owned()
            };
            out.push_str(super::login::env_note());
            Ok(out.into())
        })
    }
}
