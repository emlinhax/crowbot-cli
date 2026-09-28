#![allow(clippy::disallowed_methods)]

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// `CROWBOT_FAKE_NOW` (RFC 3339) pins the clock so tests can exercise expiry without sleeping.
pub fn now() -> Timestamp {
    crate::settings::env("CROWBOT_FAKE_NOW")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(Timestamp::now)
}

/// The user's local date, e.g. `2026-09-28`.
pub fn today() -> String {
    now().to_zoned(TimeZone::system()).date().to_string()
}

/// A monotonic instant, for measuring durations.
pub fn instant() -> std::time::Instant {
    std::time::Instant::now()
}
