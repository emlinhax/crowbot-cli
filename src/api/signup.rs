//! Account creation. crowbot asks for proof of work instead of an email: find a nonce whose
//! `sha256("challenge:nonce")` starts with `bits` zero bits.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

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
/// went stale (crowbot raised the difficulty meanwhile), up to `signup.max_attempts`.
pub async fn register(api: &Api) -> Result<Account, ApiError> {
    let mut stale = None;
    for _ in 0..limits::get().signup.max_attempts.value {
        let challenge = challenge(api).await?;
        within_bound(&challenge)?;
        let seed = challenge.challenge.clone();
        let nonce = tokio::task::spawn_blocking(move || solve(&seed, challenge.bits))
            .await
            .map_err(|e| ErrorInfo::local("unknown", e.to_string()))?;
        match create(api, &challenge.challenge, &nonce).await {
            Err(e) if e.info.kind == "pow_required" => stale = Some(e),
            done => return done,
        }
    }
    Err(stale.unwrap_or_else(|| ErrorInfo::local("pow_required", "no challenge was tried").into()))
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
    let resp = api
        .call(
            "pow",
            Call {
                args: &[("kind", "signup")],
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    parse(&resp.body)
}

async fn create(api: &Api, challenge: &str, nonce: &str) -> Result<Account, ApiError> {
    let proof = format!("{challenge}.{nonce}");
    let resp = api
        .call(
            "signup",
            Call {
                headers: &[("X-Pow", &proof)],
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    parse(&resp.body)
}

/// Searches nonces on every core; blocking, so run it off the async threads.
fn solve(challenge: &str, bits: u32) -> String {
    let prefix = Sha256::new_with_prefix(format!("{challenge}:"));
    let workers = std::thread::available_parallelism().map_or(1, usize::from) as u64;
    let found = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|start| {
                let (prefix, found) = (&prefix, &found);
                scope.spawn(move || {
                    let mut n = start;
                    while !found.load(Ordering::Relaxed) {
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
        handles
            .into_iter()
            .find_map(|h| h.join().ok().flatten())
            .expect("some worker finds a nonce")
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

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solution_meets_the_difficulty() {
        let nonce = solve("abc123", 12);
        let hash = Sha256::digest(format!("abc123:{nonce}"));
        assert!(leading_zero_bits(&hash) >= 12);
        assert!(nonce.len() <= 40);
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
