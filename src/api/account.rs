//! The signed-in account: balance, pass and spend.

use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::{Api, Call};
use crate::config::limits;

#[derive(Debug, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    api.call_json(
        "me",
        Call::default(),
        limits::get().http.request_timeout_ms.ms(),
    )
    .await
}
