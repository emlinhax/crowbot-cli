//! The signed-in account: balance and whether it can spend.

use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::{Api, Call};

#[derive(Debug, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    api.call_json("me", Call::default()).await
}
