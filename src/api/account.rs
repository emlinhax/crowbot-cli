//! The signed-in account: balance, pass and spend.

use serde::Deserialize;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

#[derive(Debug, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    let resp = api
        .call(
            "me",
            Call::default(),
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    serde_json::from_slice(&resp.body)
        .map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}
