use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::install::{self, Place};
use crate::io;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/uninstall.toml")));

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
            let exe = io::proc::current_exe()?;
            Ok(install::uninstall(&Place::here()?, &exe, &cx.app.paths)?.into())
        })
    }
}
