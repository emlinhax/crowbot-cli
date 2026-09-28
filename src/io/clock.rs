#![allow(clippy::disallowed_methods)]

use jiff::Timestamp;

/// `CROWBOT_FAKE_NOW` (RFC 3339) pins the clock so tests can exercise expiry without sleeping.
pub fn now() -> Timestamp {
    crate::settings::env("CROWBOT_FAKE_NOW")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(Timestamp::now)
}
