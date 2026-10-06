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
    /// Each `Set-Cookie` header, verbatim, so a caller that keeps a session can replay them.
    pub set_cookies: Vec<String>,
}

/// A POST, for an API that speaks over the browser transport (the forum XML-RPC and directory).
/// A page fetch stays on `get`.
pub struct Post<'a> {
    pub url: &'a str,
    pub body: Vec<u8>,
    pub content_type: &'a str,
    /// A `Cookie` header to send as-is; set, it keeps cffetch's own jar out of the request.
    pub cookie: Option<&'a str>,
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
        self.run(self.client.get(url)).await
    }

    pub async fn post(&self, req: Post<'_>) -> Result<Fetched, FetchError> {
        let mut build = self
            .client
            .post(req.url)
            .header("content-type", req.content_type)
            .body(req.body);
        if let Some(cookie) = req.cookie {
            build = build.header("cookie", cookie);
        }
        self.run(build).await
    }

    async fn run(&self, build: cffetch::CfRequestBuilder) -> Result<Fetched, FetchError> {
        let resp = tokio::time::timeout(self.timeout, build.send())
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
            set_cookies: resp.set_cookies().to_vec(),
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

    #[tokio::test]
    async fn a_post_sends_body_type_and_cookie_and_reads_set_cookie() {
        use axum::body::{Body, Bytes};
        use axum::http::HeaderMap;
        use axum::response::Response;
        use axum::routing::post;

        async fn echo(headers: HeaderMap, body: Bytes) -> Response {
            let header = |name| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
            };
            let seen = format!(
                "ct={} cookie={} body={}",
                header("content-type"),
                header("cookie"),
                String::from_utf8_lossy(&body),
            );
            Response::builder()
                .header("set-cookie", "sess=abc; Path=/")
                .header("set-cookie", "extra=1")
                .body(Body::from(seen))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/echo", listener.local_addr().unwrap());
        let router = axum::Router::new().route("/echo", post(echo));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let fetch = Fetch::new(Duration::from_secs(5)).unwrap();
        let got = fetch
            .post(Post {
                url: &url,
                body: b"<methodCall/>".to_vec(),
                content_type: "text/xml",
                cookie: Some("bbsessionhash=prev"),
            })
            .await
            .unwrap();

        assert_eq!(got.status, 200);
        let body = String::from_utf8_lossy(&got.body);
        assert!(body.contains("ct=text/xml"), "{body}");
        assert!(body.contains("cookie=bbsessionhash=prev"), "{body}");
        assert!(body.contains("body=<methodCall/>"), "{body}");
        assert_eq!(got.set_cookies, ["sess=abc; Path=/", "extra=1"]);
    }
}
