use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::io;
use crate::release::install::{self, Place};

static SPEC: LazyLock<Spec> = super::spec!("uninstall");

pub struct Uninstall;

impl Command for Uninstall {
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
            let exe = io::exe::current()?;
            Ok(install::uninstall(&Place::here()?, &exe, &cx.app.paths)?.into())
        })
    }
}
