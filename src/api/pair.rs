//! Device pairing: the machine asks for a code, a signed-in person approves it on the web,
//! and the machine collects its own revocable key.

use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::{Api, Call};
use crate::limits;

#[derive(Debug, Deserialize)]
pub struct Started {
    pub device_code: String,
    pub user_code: String,
    /// Seconds the code stays valid.
    pub ttl: u64,
}

pub enum Poll {
    Pending,
    Ready(String),
}

#[derive(Deserialize)]
struct Ready {
    api_key: String,
}

pub async fn start(api: &Api, hostname: &str) -> Result<Started, ApiError> {
    let body = serde_json::json!({ "hostname": hostname });
    let resp = api
        .call(
            "pair_start",
            Call {
                body: Some(&body),
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    serde_json::from_slice(&resp.body).map_err(|e| unreadable(&e))
}

pub async fn poll(api: &Api, device_code: &str) -> Result<Poll, ApiError> {
    let resp = api
        .call(
            "pair_poll",
            Call {
                args: &[("code", device_code)],
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    if resp.status == 202 {
        return Ok(Poll::Pending);
    }
    let ready: Ready = serde_json::from_slice(&resp.body).map_err(|e| unreadable(&e))?;
    Ok(Poll::Ready(ready.api_key))
}

fn unreadable(e: &serde_json::Error) -> ApiError {
    crate::api::error::ErrorInfo::local("bad_response", e.to_string()).into()
}
