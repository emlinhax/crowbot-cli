use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Spec};
use crate::api::endpoints;
use crate::auth;
use crate::data;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("logout");
static TEXT: LazyLock<Text> = data::load!("commands/logout.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    logged_out: String,
    no_key: String,
}

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
            let removed = auth::forget(&cx.app.api, &cx.app.paths)?;
            let mut out = if removed {
                // crowbot refuses to revoke the key making the request, so it can only go elsewhere.
                fill(&TEXT.logged_out, &[("chat", &endpoints::origin("chat"))])
            } else {
                TEXT.no_key.clone()
            };
            out.push_str(super::login::env_note());
            Ok(out.into())
        })
    }
}
