use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::release::update;

static SPEC: LazyLock<Spec> = super::spec!("update");

pub struct Update;

impl Command for Update {
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
            let outcome = update::now(&cx.app.http, &cx.app.paths).await?;
            // This run reported it; the next start need not say it again.
            update::take_news(&cx.app.paths);
            Ok(outcome.say().into())
        })
    }
}
