//! Error taxonomy for cffetch.

use thiserror::Error;

/// All ways a cffetch request can fail.
#[derive(Debug, Error)]
pub enum CfError {
    /// Transport/HTTP error from the underlying wreq client.
    #[error("http error: {0}")]
    Http(#[from] wreq::Error),

    /// Every fingerprint in the pool was met with a managed challenge.
    #[error("cloudflare challenge not bypassed after trying fingerprints: {tried:?}")]
    ChallengeFailed {
        /// Ordered fingerprints that were attempted.
        tried: Vec<String>,
        /// HTTP status of the final challenge response.
        last_status: u16,
    },

    /// The target demands an interactive Turnstile solve (Tier 5).
    ///
    /// cffetch does not execute JS; hand this off to a headless browser or
    /// solver service. `cookies` contains the session's `Set-Cookie` values
    /// (e.g. `__cf_bm`) so the browser context can continue the same session.
    #[error("interactive Cloudflare Turnstile challenge at {url} (site_key={site_key:?}); a real browser is required")]
    InteractiveChallenge {
        /// The challenged URL.
        url: String,
        /// Extracted cf-turnstile sitekey, if present.
        site_key: Option<String>,
        /// `Set-Cookie` header values seen so far (for session handoff).
        cookies: Vec<String>,
    },

    /// Client construction failed.
    #[error("client build error: {0}")]
    Build(String),
}
