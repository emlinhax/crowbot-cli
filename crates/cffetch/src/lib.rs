//! cffetch — Cloudflare-aware HTTP client with a reqwest-like interface.
//!
//! Thin intelligence layer over [`wreq`] (BoringSSL, real browser TLS):
//! fingerprint rotation on managed challenges, Turnstile detection with a
//! clean handoff error, and per-fingerprint cookie stickiness.
//!
//! ```no_run
//! use cffetch::CfClient;
//!
//! # async fn demo() -> Result<(), cffetch::CfError> {
//! let client = CfClient::builder().build()?;
//! let resp = client.get("https://community.cloudflare.com").send().await?;
//! println!("{} via {:?}", resp.status(), resp.fingerprint_used());
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod detect;
pub mod error;
pub mod fingerprint;

pub use client::{CfClient, CfClientBuilder, CfRequestBuilder, CfResponse};
pub use error::CfError;
pub use fingerprint::{Fingerprints, PickMode};

/// One-shot GET with a default client (rotation enabled).
///
/// For anything repeated, build a [`CfClient`] so cookies and connection
/// pools are reused.
pub async fn get(url: &str) -> Result<CfResponse, CfError> {
    CfClient::builder().build()?.get(url).send().await
}
