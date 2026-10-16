//! Account creation. crowbot asks for proof of work instead of an email: find a nonce whose
//! `sha256("challenge:nonce")` starts with `bits` zero bits.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::error::ApiError;
use crate::api::{Api, Call};
use crate::config::limits;
use crate::conversation::error::ErrorInfo;

#[derive(Debug, Deserialize)]
struct Challenge {
    challenge: String,
    bits: u32,
}

/// No `Debug`: it holds the account number.
#[derive(Deserialize)]
pub struct Account {
    pub account_number: String,
    pub formatted: String,
}

/// Solves a challenge and creates the account, fetching a fresh challenge when the one solved
/// went stale (crowbot raised the difficulty meanwhile), up to `signup.max_attempts`. Dropping
/// the future stops the search, so a closed card leaves no core busy.
pub async fn register(api: &Api) -> Result<Account, ApiError> {
    let stop = StopOnDrop(Arc::default());
    let mut stale = None;
    for _ in 0..limits::get().signup.max_attempts.value {
        let challenge = challenge(api).await?;
        within_bound(&challenge)?;
        let seed = challenge.challenge.clone();
        let flag = Arc::clone(&stop.0);
        let nonce = tokio::task::spawn_blocking(move || solve(&seed, challenge.bits, &flag))
            .await
            .map_err(|e| ErrorInfo::local("unknown", e.to_string()))?
            .ok_or_else(|| ErrorInfo::local("unknown", "the search was stopped"))?;
        match create(api, &challenge.challenge, &nonce).await {
            Err(e) if e.info.kind == "pow_required" => stale = Some(e),
            done => return done,
        }
    }
    Err(stale.unwrap_or_else(|| ErrorInfo::local("pow_required", "no challenge was tried").into()))
}

/// Raises its flag when dropped, with the future that holds it.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn within_bound(challenge: &Challenge) -> Result<(), ApiError> {
    let max = limits::get().signup.max_bits.value;
    if challenge.bits > max {
        return Err(ErrorInfo::local(
            "pow_too_hard",
            format!(
                "the challenge needs {} bits; this machine stops at {max}",
                challenge.bits
            ),
        )
        .into());
    }
    Ok(())
}

async fn challenge(api: &Api) -> Result<Challenge, ApiError> {
    api.call_json(
        "pow",
        Call {
            args: &[("kind", "signup")],
            ..Call::default()
        },
    )
    .await
}

async fn create(api: &Api, challenge: &str, nonce: &str) -> Result<Account, ApiError> {
    let proof = format!("{challenge}.{nonce}");
    api.call_json(
        "signup",
        Call {
            headers: &[("X-Pow", &proof)],
            ..Call::default()
        },
    )
    .await
}

/// Searches nonces on every core until one is found or `stop` is raised (`None`); blocking, so
/// run it off the async threads.
fn solve(challenge: &str, bits: u32, stop: &AtomicBool) -> Option<String> {
    let prefix = Sha256::new_with_prefix(format!("{challenge}:"));
    let workers = std::thread::available_parallelism().map_or(1, usize::from) as u64;
    let found = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|start| {
                let (prefix, found) = (&prefix, &found);
                scope.spawn(move || {
                    let mut n = start;
                    while !found.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                        let nonce = base36(n);
                        let mut hash = prefix.clone();
                        hash.update(nonce.as_bytes());
                        if leading_zero_bits(&hash.finalize()) >= bits {
                            found.store(true, Ordering::Relaxed);
                            return Some(nonce);
                        }
                        n += workers;
                    }
                    None
                })
            })
            .collect();
        handles.into_iter().find_map(|h| h.join().ok().flatten())
    })
}

fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        bits += byte.leading_zeros();
        if *byte != 0 {
            break;
        }
    }
    bits
}

/// Nonces stay short and free of the `.` that separates them from the challenge.
fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII digits")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solution_meets_the_difficulty() {
        let nonce = solve("abc123", 12, &AtomicBool::new(false)).unwrap();
        let hash = Sha256::digest(format!("abc123:{nonce}"));
        assert!(leading_zero_bits(&hash) >= 12);
        assert!(nonce.len() <= 40);
    }

    #[test]
    fn a_raised_stop_ends_the_search_unsolved() {
        // 256 bits is never found, so only the flag can end it.
        assert_eq!(solve("abc123", 256, &AtomicBool::new(true)), None);
        let stop = StopOnDrop(Arc::default());
        let flag = Arc::clone(&stop.0);
        drop(stop);
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn a_challenge_past_the_bound_is_refused_unsolved() {
        let max = limits::get().signup.max_bits.value;
        let challenge = |bits| Challenge {
            challenge: "abc".into(),
            bits,
        };
        assert!(within_bound(&challenge(max)).is_ok());
        let err = within_bound(&challenge(max + 1)).unwrap_err();
        assert_eq!(err.info.kind, "pow_too_hard");
    }

    #[test]
    fn counts_zero_bits_across_bytes() {
        assert_eq!(leading_zero_bits(&[0, 0b0001_0000, 0xff]), 11);
        assert_eq!(leading_zero_bits(&[0xff]), 0);
    }

    #[test]
    fn base36_digits() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }
}
