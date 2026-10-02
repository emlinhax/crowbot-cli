use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Session, Spec, login};
use crate::api::account::{self, Me};
use crate::api::error::ApiError;
use crate::text::template::fill;
use crate::text::units;

const SRC: &str = include_str!("../../data/commands/status.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/status.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    body: String,
    balance: String,
    balance_failed: String,
    effort: String,
    context: String,
    no_context: String,
}

pub struct Status;

impl Command for Status {
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
            let session = cx.session.as_ref().ok_or_else(|| cx.usage(&SPEC))?;
            let login = login::status(cx)?;
            let balance = match cx.app.api.has_key() {
                true => Some(account::me(&cx.app.api).await),
                false => None,
            };
            let project = cx.app.paths.project.display().to_string();
            Ok(render(session, &login, balance, &project).into())
        })
    }
}

/// `balance` is `None` when there is no key to ask with.
fn render(
    session: &Session,
    login: &str,
    balance: Option<Result<Me, ApiError>>,
    project: &str,
) -> String {
    let account = match balance {
        None => login.to_owned(),
        Some(Ok(me)) => fill(
            &TEXT.balance,
            &[
                ("login", login),
                ("balance", &units::usd_micros(me.balance_microdollars)),
            ],
        ),
        Some(Err(e)) => fill(
            &TEXT.balance_failed,
            &[("login", login), ("error", &e.to_string())],
        ),
    };
    let model = match &session.effort {
        Some(effort) => fill(
            &TEXT.effort,
            &[("model", &session.model), ("effort", effort)],
        ),
        None => session.model.clone(),
    };
    let context = match session.context_pct {
        Some(pct) => fill(&TEXT.context, &[("pct", &pct.to_string())]),
        None => TEXT.no_context.clone(),
    };
    fill(
        &TEXT.body,
        &[
            ("account", &account),
            ("model", &model),
            ("mode", &session.mode),
            ("context", &context),
            ("cost", &units::usd_micros(session.cost_micros)),
            ("file", &session.file),
            ("project", project),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            model: "crow-2".into(),
            effort: Some("high".into()),
            mode: "MANUAL".into(),
            context_pct: Some(12),
            cost_micros: 42_000,
            file: "/home/u/.crowbot/sessions/s.jsonl".into(),
        }
    }

    #[test]
    fn it_names_the_account_model_mode_context_and_cost() {
        let me = Me {
            balance_microdollars: 12_300_000,
            can_spend: true,
        };
        let text = render(
            &session(),
            "Logged in with device key …9999.",
            Some(Ok(me)),
            "/p",
        );
        for want in [
            "Account: Logged in with device key …9999. Balance $12.30.",
            "Model: crow-2, effort high",
            "Mode: MANUAL",
            "Context: 12% of the window",
            "This session: about $0.042",
            "`/home/u/.crowbot/sessions/s.jsonl`",
        ] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
        let quiet = Session {
            effort: None,
            context_pct: None,
            ..session()
        };
        let text = render(&quiet, "Not logged in.", None, "/p");
        assert!(text.contains("Account: Not logged in.\n"), "{text}");
        assert!(
            text.contains("Model: crow-2\n") && text.contains("nothing sent yet"),
            "{text}"
        );
    }
}
