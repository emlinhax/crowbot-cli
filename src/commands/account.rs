use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::api::account::Me;
use crate::api::endpoints;
use crate::api::error::ApiError;
use crate::data;
use crate::text::template::fill;
use crate::text::units;

static SPEC: LazyLock<Spec> = super::spec!("account");
static TEXT: LazyLock<Text> = data::load!("commands/account.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    balance: String,
    plan: String,
    refill: String,
    fallback_off: String,
    cannot_spend: String,
}

pub struct Account;

impl Command for Account {
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
            if !cx.app.api.has_key() {
                return Err(ApiError::not_logged_in().into());
            }
            Ok(Effect::Account.into())
        })
    }
}

/// Where the account stands, in one sentence or a few: balance, plan, and what stops it.
pub fn standing(me: &Me) -> String {
    let mut out = fill(
        &TEXT.balance,
        &[("balance", &units::usd_micros(me.balance_microdollars))],
    );
    if let Some(pass) = &me.pass {
        // CEILING: `refills` is crowbot's own English phrase. Upgrade: word `full_in_s` here
        // once the words are localised.
        let refill = match &pass.refills {
            Some(refills) => fill(&TEXT.refill, &[("refills", refills)]),
            None => String::new(),
        };
        out.push_str(&fill(
            &TEXT.plan,
            &[
                ("plan", &pass.label),
                ("pct", &pass.pct().to_string()),
                ("refill", &refill),
            ],
        ));
        if me.spend_balance_when_dry == Some(false) {
            out.push_str(&TEXT.fallback_off);
        }
    }
    if !me.can_spend {
        out.push(' ');
        out.push_str(&cannot_spend());
    }
    out
}

/// That the account cannot spend, and where to fix it.
pub fn cannot_spend() -> String {
    fill(&TEXT.cannot_spend, &[("chat", &endpoints::origin("chat"))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::account::Pass;

    fn funded() -> Me {
        Me {
            balance_microdollars: 12_300_000,
            can_spend: true,
            ..Me::default()
        }
    }

    fn pro(allowance_bp: u32, refills: Option<&str>) -> Option<Pass> {
        Some(Pass {
            label: "Pro".into(),
            allowance_bp,
            refills: refills.map(str::to_owned),
        })
    }

    #[test]
    fn it_says_the_balance_the_plan_and_what_stops_spending() {
        assert_eq!(standing(&funded()), "Balance $12.30.");
        let full = Me {
            pass: pro(10_000, None),
            ..funded()
        };
        assert_eq!(
            standing(&full),
            "Balance $12.30. Plan Pro: 100% of its allowance left."
        );
        let partial = Me {
            pass: pro(6_399, Some("3 hours")),
            spend_balance_when_dry: Some(false),
            ..funded()
        };
        let text = standing(&partial);
        assert!(
            text.starts_with("Balance $12.30. Plan Pro: 63% of its allowance left, full in 3 hours. Balance fallback is off"),
            "{text}"
        );
        let dry = Me {
            can_spend: false,
            balance_microdollars: 0,
            ..funded()
        };
        let text = standing(&dry);
        assert!(
            text.starts_with("Balance $0.00. The account cannot spend yet"),
            "{text}"
        );
        assert!(!text.contains('{'), "{text}");
    }
}
