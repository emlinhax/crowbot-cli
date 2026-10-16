//! The signed-in account: balance, plan, and whether it can spend.

use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::{Api, Call};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
    #[serde(default)]
    pub pass: Option<Pass>,
    /// Whether the balance pays once the plan's allowance is spent. `None` when the server does
    /// not say, which is not the same as off: crowbot's default is on.
    #[serde(default)]
    pub spend_balance_when_dry: Option<bool>,
}

/// The plan the account holds, as its holder may see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Pass {
    #[serde(default)]
    pub label: String,
    /// What is left of the allowance, in hundredths of a percent.
    #[serde(default)]
    pub allowance_bp: u32,
    /// How long until it is full again, in crowbot's words ("3 hours"); `None` when full.
    #[serde(default)]
    pub refills: Option<String>,
}

impl Pass {
    /// The share left in whole percent, rounded down so it is never overstated.
    pub fn pct(&self) -> u32 {
        self.allowance_bp.min(10_000) / 100
    }
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    api.call_json("me", Call::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_reads_the_full_reply_and_the_old_two_fields() {
        let full = r#"{"tenant_id": 7, "balance_microdollars": 12300000, "balance_usd": "12.30",
            "can_spend": true, "has_pass": true, "spend_balance_when_dry": false,
            "pass": {"plan_id": "pro", "label": "Pro", "flair": null, "scope": [],
                     "ends_at": "2026-11-01T00:00:00+00:00", "allowance_bp": 6375,
                     "refills": "3 hours", "refill_bp_per_hour": 1200, "full_in_s": 10800}}"#;
        let me: Me = serde_json::from_str(full).unwrap();
        let pass = me.pass.as_ref().unwrap();
        assert_eq!((pass.label.as_str(), pass.pct()), ("Pro", 63));
        assert_eq!(pass.refills.as_deref(), Some("3 hours"));
        assert_eq!(me.spend_balance_when_dry, Some(false));

        let old: Me =
            serde_json::from_str(r#"{"balance_microdollars": 5, "can_spend": true}"#).unwrap();
        assert_eq!((old.pass, old.spend_balance_when_dry), (None, None));
    }
}
