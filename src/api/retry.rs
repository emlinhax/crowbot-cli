use std::time::Duration;

use crate::api::error::ErrorInfo;
use crate::limits;

/// How long to wait before retrying after `attempt` failed with `error`, or `None` to give up.
pub fn delay(error: &ErrorInfo, retry_after: Option<Duration>, attempt: u32) -> Option<Duration> {
    let policy = &limits::get().retry;
    if attempt >= policy.max_attempts.value || !retryable(error) {
        return None;
    }
    if let Some(wait) = retry_after {
        return (wait <= policy.max_retry_after_secs.secs()).then_some(wait);
    }
    let base = policy
        .initial_delay_ms
        .value
        .saturating_mul(u64::from(policy.factor.value).saturating_pow(attempt - 1))
        .min(policy.max_delay_ms.value);
    let spread = base * policy.jitter_pct.value / 100;
    let jittered = base - spread + fastrand::u64(0..=spread * 2);
    Some(Duration::from_millis(jittered))
}

/// The catalog's flag, where it has one; otherwise rate limits and server errors are worth
/// another try, and client errors would fail the same way again.
pub fn retryable(error: &ErrorInfo) -> bool {
    error
        .entry()
        .retry
        .unwrap_or(matches!(error.status, Some(429 | 500..=599)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(kind: &str, status: Option<u16>) -> ErrorInfo {
        ErrorInfo {
            status,
            ..ErrorInfo::local(kind, "x")
        }
    }

    #[test]
    fn backs_off_with_jitter_inside_bounds() {
        let err = error("upstream_unavailable", Some(503));
        for attempt in 1..4 {
            let d = delay(&err, None, attempt).unwrap().as_millis() as u64;
            let base = 2000 * 2u64.pow(attempt - 1);
            assert!(
                d >= base * 3 / 4 && d <= base * 5 / 4,
                "attempt {attempt}: {d}ms"
            );
        }
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let max = limits::get().retry.max_attempts.value;
        assert!(delay(&error("network", None), None, max).is_none());
    }

    #[test]
    fn honours_retry_after_up_to_the_cap() {
        let err = error("rate_limited", Some(429));
        assert_eq!(
            delay(&err, Some(Duration::from_secs(4)), 1),
            Some(Duration::from_secs(4))
        );
        assert_eq!(delay(&err, Some(Duration::from_secs(3600)), 1), None);
    }

    #[test]
    fn does_not_retry_what_will_fail_again() {
        assert!(delay(&error("insufficient_balance", Some(402)), None, 1).is_none());
        assert!(delay(&error("http_error", Some(404)), None, 1).is_none());
        assert!(delay(&error("http_error", Some(502)), None, 1).is_some());
        // A vendor's own status passes through: its 400 is final, its 429 is not.
        assert!(delay(&error("upstream_error", Some(400)), None, 1).is_none());
        assert!(delay(&error("upstream_error", Some(429)), None, 1).is_some());
    }

    #[test]
    fn the_catalog_flag_decides_and_the_status_only_fills_in() {
        assert!(!retryable(&error("pow_required", Some(428))));
        assert!(!retryable(&error("no_provider", Some(503))));
        assert!(retryable(&error("upstream_unavailable", Some(503))));
        assert!(retryable(&error("network", None)));
        assert!(retryable(&error("a_kind_from_a_newer_crowbot", Some(503))));
        assert!(!retryable(&error("a_kind_from_a_newer_crowbot", Some(409))));
        assert!(!retryable(&error("bad_stream", None)));
    }
}
