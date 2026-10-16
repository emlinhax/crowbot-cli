//! The signed-in account: balance, plan, whether it can spend, and its keys.

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

/// One key that signs in to the account. No secret: crowbot keeps only its hint.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Key {
    /// What revoking names it by.
    #[serde(rename = "ref")]
    pub reference: String,
    /// `account` for the account number itself, else the paired device's name.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub created_at: String,
    /// The key this request signed in with.
    #[serde(default)]
    pub current: bool,
}

impl Key {
    /// The account number itself, rather than a paired device's key.
    pub fn is_account(&self) -> bool {
        self.name == "account"
    }
}

#[derive(Deserialize)]
struct Keys {
    keys: Vec<Key>,
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    api.call_json("me", Call::default()).await
}

pub async fn keys(api: &Api) -> Result<Vec<Key>, ApiError> {
    let keys: Keys = api.call_json("me_keys", Call::default()).await?;
    Ok(keys.keys)
}

pub async fn revoke(api: &Api, key: &Key) -> Result<(), ApiError> {
    let call = Call {
        args: &[("ref", &key.reference)],
        ..Call::default()
    };
    api.call("me_key_revoke", call).await.map(drop)
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

    #[tokio::test]
    async fn keys_are_listed_and_one_is_revoked_by_its_reference() {
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        let base = crate::testing::serve(|_| {
            axum::Router::new()
                .route(
                    "/api/me/keys",
                    get(|| async {
                        axum::Json(serde_json::json!({"keys": [
                            {"ref": "0123456789ab", "name": "laptop", "hint": "•••• 9999",
                             "created_at": "2026-10-01T09:00:00+00:00", "current": false}]}))
                    }),
                )
                .route(
                    "/api/me/keys/{reference}/revoke",
                    post(
                        |axum::extract::Path(r): axum::extract::Path<String>| async move {
                            match r.as_str() {
                                "0123456789ab" => {
                                    axum::Json(serde_json::json!({"ok": true})).into_response()
                                }
                                _ => (
                                    axum::http::StatusCode::CONFLICT,
                                    r#"{"error":{"message":"no","type":"cannot_revoke_current"}}"#,
                                )
                                    .into_response(),
                            }
                        },
                    ),
                )
        })
        .await;
        let http = crate::io::http::Http::new(std::time::Duration::from_secs(5)).unwrap();
        let api = Api::redirected(http, "k", &base);
        let listed = keys(&api).await.unwrap();
        assert_eq!(listed[0].name, "laptop");
        assert!(!listed[0].is_account());
        revoke(&api, &listed[0]).await.unwrap();
        let mine = Key {
            reference: "ffffffffffff".into(),
            ..listed[0].clone()
        };
        let refused = revoke(&api, &mine).await.unwrap_err();
        assert_eq!(refused.info.kind, "cannot_revoke_current");
    }
}
