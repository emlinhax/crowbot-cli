use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Outcome, Spec};
use crate::io;
use crate::release::install::{self, Place};

static SPEC: LazyLock<Spec> = super::spec!("install");

pub struct Install;

impl Command for Install {
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
            Ok(install::install(&Place::here()?, &exe)?.into())
        })
    }
}
