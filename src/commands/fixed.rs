//! Commands that take no arguments and only ask the session for one effect. A new one is a
//! spec file in data/commands/ and a static here.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::data;

pub struct Fixed {
    spec: LazyLock<Spec>,
    effect: fn() -> Effect,
}

impl Command for Fixed {
    fn spec(&self) -> &Spec {
        &self.spec
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&self.spec, args)?;
            Ok((self.effect)().into())
        })
    }
}

pub static COPY: Fixed = Fixed {
    spec: super::spec!("copy"),
    effect: || Effect::CopyLastReply,
};

pub static INIT: Fixed = Fixed {
    spec: super::spec!("init"),
    effect: || Effect::Send(data::raw!("prompts/init.md").trim_end().to_owned()),
};

pub static NEW: Fixed = Fixed {
    spec: super::spec!("new"),
    effect: || Effect::NewConversation,
};

pub static RESUME: Fixed = Fixed {
    spec: super::spec!("resume"),
    effect: || Effect::Resume,
};

pub static UNDO: Fixed = Fixed {
    spec: super::spec!("undo"),
    effect: || Effect::Undo,
};

pub static RETRY: Fixed = Fixed {
    spec: super::spec!("retry"),
    effect: || Effect::Retry,
};

pub static QUIT: Fixed = Fixed {
    spec: super::spec!("quit"),
    effect: || Effect::Quit,
};
