use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// `CROWBOT_FAKE_NOW` (RFC 3339) pins the clock so tests can exercise expiry without sleeping.
pub fn now() -> Timestamp {
    super::env::var("CROWBOT_FAKE_NOW")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(Timestamp::now)
}

/// The user's local date, e.g. `2026-09-28`.
pub fn today() -> String {
    now().to_zoned(TimeZone::system()).date().to_string()
}

/// Whether something stored `at` is younger than `ttl`. A time ahead of `now` (written while the
/// clock ran ahead) is not fresh, so it never holds off a refresh.
pub fn fresh(at: Timestamp, now: Timestamp, ttl: SignedDuration) -> bool {
    let age = now.duration_since(at);
    age >= SignedDuration::ZERO && age < ttl
}

pub async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

/// `future`'s output, or `Err` once `duration` passes first.
pub async fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(duration, future).await
}

/// A tick every `period`; ticks missed while busy are skipped, not caught up in a burst.
pub fn ticks(period: Duration) -> tokio::time::Interval {
    let mut ticks = tokio::time::interval(period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticks
}

/// A monotonic instant, for measuring durations.
pub fn instant() -> std::time::Instant {
    std::time::Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_respects_ttl_and_clock_skew() {
        let t: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let ttl = SignedDuration::from_secs(60);
        assert!(fresh(t, t + SignedDuration::from_secs(59), ttl));
        assert!(!fresh(t, t + SignedDuration::from_secs(60), ttl));
        assert!(!fresh(t, t - SignedDuration::from_secs(1), ttl));
    }
}
