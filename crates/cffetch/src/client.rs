//! Core client: CfClient with fingerprint rotation over wreq.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Mutex;
use wreq::header::HeaderMap;
use wreq::{Client, Method, StatusCode, Uri};

use crate::detect;
use crate::error::CfError;
use crate::fingerprint::{FingerprintPicker, Fingerprints, PickMode};
use wreq_util::Profile;

/// A buffered response with Cloudflare metadata.
///
/// Note: unlike the Python port, there is no `history()` accessor — wreq
/// follows redirects internally without exposing the chain (same guarantee
/// either way: the final response's fingerprint is the one that mattered).
pub struct CfResponse {
    status: StatusCode,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    fingerprint: String,
    set_cookies: Vec<String>,
}

impl CfResponse {
    /// HTTP status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Final URI after redirects.
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Raw body bytes.
    pub fn bytes(&self) -> &Bytes {
        &self.body
    }

    /// Body as UTF-8 lossy text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Deserialize the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// The fingerprint that produced this response.
    pub fn fingerprint_used(&self) -> &str {
        &self.fingerprint
    }

    /// True when Cloudflare served a JS/managed challenge page.
    pub fn is_cf_challenge(&self) -> bool {
        detect::is_managed_challenge(self.status.as_u16(), &self.headers, &self.body)
    }

    /// True when the page requires an interactive Turnstile solve.
    pub fn needs_browser(&self) -> bool {
        detect::is_interactive_turnstile(&self.body)
    }

    /// True for CF's classic hard WAF block page (fix the request, don't rotate).
    pub fn is_waf_block(&self) -> bool {
        detect::is_waf_block(&self.body)
    }

    /// `Set-Cookie` values seen on this response (for browser handoff).
    pub fn set_cookies(&self) -> &[String] {
        &self.set_cookies
    }
}

/// Builder for [`CfClient`].
pub struct CfClientBuilder {
    fingerprints: Fingerprints,
    fingerprint_mode: PickMode,
    rotate_on_challenge: bool,
    max_retries: usize,
    timeout: Duration,
    cookie_store: bool,
    /// Whether the pool came from [`Fingerprints::auto`] (drives
    /// deterministic challenge fallback). Set by builder methods.
    auto_pool: bool,
}

impl CfClientBuilder {
    /// Fingerprint pool. Default: [`Fingerprints::auto`].
    pub fn fingerprints(mut self, fp: Fingerprints) -> Self {
        self.auto_pool = false; // explicit pool = user order governs fallback
        self.fingerprints = fp;
        self
    }

    /// Fingerprint selection strategy (static/rotate/random per request).
    /// Default: [`PickMode::Static`].
    pub fn fingerprint_mode(mut self, mode: PickMode) -> Self {
        self.fingerprint_mode = mode;
        self
    }

    /// Rotate to the next fingerprint when challenged. Default: true.
    pub fn rotate_on_challenge(mut self, yes: bool) -> Self {
        self.rotate_on_challenge = yes;
        self
    }

    /// Max extra attempts after the first. Default: 2.
    pub fn max_retries(mut self, n: usize) -> Self {
        self.max_retries = n;
        self
    }

    /// Per-request timeout. Default: 15s.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Keep cookies (`__cf_bm`, `cf_clearance`) across requests. Default: true.
    pub fn cookie_store(mut self, yes: bool) -> Self {
        self.cookie_store = yes;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<CfClient, CfError> {
        if self.fingerprints.0.is_empty() {
            return Err(CfError::Build(
                "fingerprint pool is empty; refusing to build a client that \
                 would silently fall back to defaults"
                    .into(),
            ));
        }
        Ok(CfClient {
            inner: Arc::new(Inner {
                picker: FingerprintPicker::with_pool_kind(
                    self.fingerprints.0,
                    self.fingerprint_mode,
                    self.auto_pool,
                ),
                rotate_on_challenge: self.rotate_on_challenge,
                max_retries: self.max_retries,
                timeout: self.timeout,
                cookie_store: self.cookie_store,
                clients: Mutex::new(HashMap::new()),
            }),
        })
    }
}

struct Inner {
    picker: FingerprintPicker,
    rotate_on_challenge: bool,
    max_retries: usize,
    timeout: Duration,
    cookie_store: bool,
    // One wreq client per fingerprint so cookie jars and connection pools
    // stay fingerprint-stable across requests.
    clients: Mutex<HashMap<String, Client>>,
}

/// Cloudflare-aware HTTP client. Clone-cheap (Arc inside).
#[derive(Clone)]
pub struct CfClient {
    inner: Arc<Inner>,
}

impl CfClient {
    /// Start building a client with default settings.
    pub fn builder() -> CfClientBuilder {
        CfClientBuilder {
            fingerprints: Fingerprints::auto(),
            fingerprint_mode: PickMode::Static,
            rotate_on_challenge: true,
            max_retries: 2,
            timeout: Duration::from_secs(15),
            cookie_store: true,
            auto_pool: true,
        }
    }

    async fn client_for(&self, profile: Profile) -> Result<(String, Client), CfError> {
        let name = format!("{profile:?}");
        let mut guard = self.inner.clients.lock().await;
        if let Some(c) = guard.get(&name) {
            return Ok((name, c.clone()));
        }
        let emulation = Fingerprints::emulation_of(&profile);
        let client = Client::builder()
            .emulation(emulation)
            .cookie_store(self.inner.cookie_store)
            .timeout(self.inner.timeout)
            .build()
            .map_err(|e| CfError::Build(e.to_string()))?;
        guard.insert(name.clone(), client.clone());
        Ok((name, client))
    }

    /// Drop a fingerprint's cached client (used after a failed challenge so
    /// the tainted cookie jar is not reused on caller-driven retries).
    async fn evict(&self, name: &str) {
        self.inner.clients.lock().await.remove(name);
    }

    /// Begin a GET request.
    pub fn get(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::GET, url)
    }

    /// Begin a POST request.
    pub fn post(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::POST, url)
    }

    /// Begin a PUT request.
    pub fn put(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::PUT, url)
    }

    /// Begin a PATCH request.
    pub fn patch(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::PATCH, url)
    }

    /// Begin a DELETE request.
    pub fn delete(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::DELETE, url)
    }

    /// Begin a HEAD request.
    pub fn head(&self, url: &str) -> CfRequestBuilder {
        self.request(Method::HEAD, url)
    }

    /// Begin a request with an arbitrary method.
    pub fn request(&self, method: Method, url: &str) -> CfRequestBuilder {
        CfRequestBuilder {
            client: self.clone(),
            method,
            url: url.to_string(),
            body: None,
            json: None,
            headers: HeaderMap::new(),
        }
    }

    async fn execute(
        &self,
        method: Method,
        url: &str,
        body: Option<Bytes>,
        json: Option<Bytes>,
        extra_headers: HeaderMap,
    ) -> Result<CfResponse, CfError> {
        // First attempt: the picker's choice (static/rotate/random).
        // Challenge fallback then walks the validated pool.
        let first = self.inner.picker.pick();
        let mut order = vec![first];
        order.extend(self.inner.picker.fallback_order(first));
        let attempts = order.len().min(1 + self.inner.max_retries);

        let mut tried: Vec<String> = Vec::new();
        let mut last_status = 0u16;

        for profile in order.into_iter().take(attempts) {
            let (fp_name, client) = self.client_for(profile).await?;
            tried.push(fp_name.clone());

            let mut req = client.request(method.clone(), url);
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            if let Some(j) = &json {
                req = req
                    .header(wreq::header::CONTENT_TYPE, "application/json")
                    .body(j.clone());
            }
            for (k, v) in extra_headers.iter() {
                req = req.header(k, v);
            }

            let resp = req.send().await?;
            let status = resp.status();
            let headers = resp.headers().clone();
            let final_uri = resp.uri().clone();
            let set_cookies: Vec<String> = headers
                .get_all(wreq::header::SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(String::from))
                .collect();
            let body_bytes = resp.bytes().await?;

            let cf_resp = CfResponse {
                status,
                headers,
                uri: final_uri,
                body: body_bytes,
                fingerprint: fp_name.clone(),
                set_cookies,
            };

            if !cf_resp.is_cf_challenge() {
                return Ok(cf_resp);
            }

            last_status = status.as_u16();

            if cf_resp.needs_browser() {
                return Err(CfError::InteractiveChallenge {
                    url: cf_resp.uri.to_string(),
                    site_key: detect::extract_turnstile_sitekey(&cf_resp.body),
                    cookies: cf_resp.set_cookies.clone(),
                });
            }

            // The session is tainted (failed challenge): evict so future
            // caller retries start from a clean jar.
            self.evict(&fp_name).await;

            if !self.inner.rotate_on_challenge {
                return Ok(cf_resp);
            }
        }

        Err(CfError::ChallengeFailed {
            tried,
            last_status,
        })
    }
}

/// A prepared request; mirror of reqwest's RequestBuilder essentials.
pub struct CfRequestBuilder {
    client: CfClient,
    method: Method,
    url: String,
    body: Option<Bytes>,
    json: Option<Bytes>,
    headers: HeaderMap,
}

impl CfRequestBuilder {
    /// Attach a raw body.
    pub fn body<B: Into<Bytes>>(mut self, body: B) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Attach a JSON-serializable body.
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Self {
        self.json = serde_json::to_vec(value).ok().map(Bytes::from);
        self
    }

    /// Add a header.
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: TryInto<wreq::header::HeaderName>,
        V: TryInto<wreq::header::HeaderValue>,
    {
        if let (Ok(k), Ok(v)) = (key.try_into(), value.try_into()) {
            self.headers.insert(k, v);
        }
        self
    }

    /// Send the request (with fingerprint rotation as configured).
    pub async fn send(self) -> Result<CfResponse, CfError> {
        self.client
            .execute(self.method, &self.url, self.body, self.json, self.headers)
            .await
    }
}
