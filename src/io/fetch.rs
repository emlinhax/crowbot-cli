#![allow(clippy::disallowed_methods)]
//! The rest of the web, for webfetch: pages fetched the way a browser fetches them (real browser
//! TLS through cffetch, another profile when one is challenged), with Cloudflare challenges told
//! apart from real pages. crowbot's own API stays on `io::http`.

use std::time::Duration;

use cffetch::{CfClient, CfError};

/// One client for the session, so each browser profile keeps its cookies between fetches.
#[derive(Clone)]
pub struct Fetch {
    client: CfClient,
    timeout: Duration,
}

pub struct Fetched {
    pub status: u16,
    /// Where redirects ended.
    pub url: String,
    /// A redirect's target as sent, possibly relative; the client does not follow it.
    pub location: Option<String>,
    /// CEILING: cffetch buffers the whole body before returning, so a size cap applies after the
    /// fetch rather than mid-stream; a streaming cap would be a change to cffetch.
    pub body: Vec<u8>,
    /// The browser profile that got the page.
    pub fingerprint: String,
    /// Cloudflare's firewall refused the request outright, which no retry changes.
    pub firewall: bool,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("every browser profile was challenged ({}), last with HTTP {last_status}", tried.join(", "))]
    Challenge {
        tried: Vec<String>,
        last_status: u16,
    },
    #[error("{url} asks for an interactive check that needs a real browser")]
    NeedsBrowser { url: String },
    #[error("timed out")]
    Timeout,
    #[error("{0}")]
    Transport(String),
}

impl From<CfError> for FetchError {
    fn from(e: CfError) -> Self {
        match e {
            CfError::ChallengeFailed { tried, last_status } => {
                Self::Challenge { tried, last_status }
            }
            CfError::InteractiveChallenge { url, .. } => Self::NeedsBrowser { url },
            CfError::Http(e) if e.is_timeout() => Self::Timeout,
            other => Self::Transport(other.to_string()),
        }
    }
}

impl Fetch {
    /// `timeout` bounds a whole fetch, every browser profile it tries included.
    pub fn new(timeout: Duration) -> Result<Self, FetchError> {
        let client = CfClient::builder().timeout(timeout).build()?;
        Ok(Self { client, timeout })
    }

    pub async fn get(&self, url: &str) -> Result<Fetched, FetchError> {
        let resp = tokio::time::timeout(self.timeout, self.client.get(url).send())
            .await
            .map_err(|_| FetchError::Timeout)??;
        let status = resp.status().as_u16();
        Ok(Fetched {
            status,
            url: resp.uri().to_string(),
            location: resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            firewall: status >= 400 && resp.is_waf_block(),
            fingerprint: resp.fingerprint_used().to_owned(),
            body: resp.bytes().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cffetch_errors_map_onto_what_webfetch_tells_the_model() {
        let challenged = CfError::ChallengeFailed {
            tried: vec!["Chrome131".into(), "Chrome124".into()],
            last_status: 403,
        };
        assert_eq!(
            FetchError::from(challenged),
            FetchError::Challenge {
                tried: vec!["Chrome131".into(), "Chrome124".into()],
                last_status: 403
            }
        );
        let interactive = CfError::InteractiveChallenge {
            url: "https://example.com/".into(),
            site_key: Some("0x4AAAAAAADnPIDROrmt1Wj0".into()),
            cookies: vec!["__cf_bm=x".into()],
        };
        assert_eq!(
            FetchError::from(interactive),
            FetchError::NeedsBrowser {
                url: "https://example.com/".into()
            }
        );
        assert!(matches!(
            FetchError::from(CfError::Build("no pool".into())),
            FetchError::Transport(m) if m.contains("no pool")
        ));
    }

    #[test]
    fn the_client_builds_without_the_network() {
        assert!(Fetch::new(Duration::from_secs(1)).is_ok());
    }
}
