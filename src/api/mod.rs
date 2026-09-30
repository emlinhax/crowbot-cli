//! crowbot's HTTP surface: endpoint catalog, typed errors, and one call path for every request.

pub mod account;
pub mod assemble;
pub mod chat;
pub mod endpoints;
pub mod error;
pub mod models;
pub mod pair;
pub mod retry;
pub mod signup;
pub mod sse;
pub mod wire;

use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::io::http::{Http, Request, Response, Streaming};
use error::ApiError;

#[derive(Clone)]
pub struct Api {
    http: Http,
    /// Shared by every clone, so logging in or out mid-session reaches all of them at once.
    key: Arc<RwLock<Option<String>>>,
    /// Replaces every origin; lets unit tests talk to a local fake.
    #[cfg(test)]
    base: Option<String>,
}

/// One call: the endpoint id from data/endpoints.toml and what fills it in.
#[derive(Default)]
pub struct Call<'a> {
    /// Values for the endpoint path's `{placeholders}`.
    pub args: &'a [(&'a str, &'a str)],
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a serde_json::Value>,
}

impl Api {
    pub fn new(http: Http, key: Option<String>) -> Self {
        Self {
            http,
            key: Arc::new(RwLock::new(key)),
            #[cfg(test)]
            base: None,
        }
    }

    pub fn has_key(&self) -> bool {
        self.key().is_some()
    }

    /// Replaces the key for this client and every clone of it.
    pub fn set_key(&self, key: Option<String>) {
        *self.key.write().unwrap_or_else(|e| e.into_inner()) = key;
    }

    /// A separate client acting as another key, e.g. to check a key before saving it.
    pub fn with_key(&self, key: String) -> Self {
        Self {
            key: Arc::new(RwLock::new(Some(key))),
            ..self.clone()
        }
    }

    fn key(&self) -> Option<String> {
        self.key.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    #[cfg(test)]
    pub fn redirected(http: Http, key: &str, base: &str) -> Self {
        Self {
            http,
            key: Arc::new(RwLock::new(Some(key.to_owned()))),
            base: Some(base.to_owned()),
        }
    }

    pub async fn call(
        &self,
        id: &str,
        call: Call<'_>,
        timeout: Duration,
    ) -> Result<Response, ApiError> {
        let resp = self.http.send(self.request(id, &call, timeout)?).await?;
        if resp.head.status >= 400 {
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }

    /// Like `call`, but hands back the body as it streams. An error's body is read first, as far
    /// as it comes within the error bounds in data/limits.toml.
    pub async fn open(
        &self,
        id: &str,
        call: Call<'_>,
        timeout: Duration,
    ) -> Result<Streaming, ApiError> {
        let resp = self.http.open(self.request(id, &call, timeout)?).await?;
        if resp.head.status >= 400 {
            let limits = &crate::limits::get().http;
            let resp = resp
                .collect(limits.error_body_bytes.value, limits.error_body_ms.ms())
                .await;
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }

    fn request<'a>(
        &'a self,
        id: &str,
        call: &Call<'a>,
        timeout: Duration,
    ) -> Result<Request<'a>, ApiError> {
        let endpoint = endpoints::get(id);
        let bearer = match (endpoint.auth, self.key()) {
            (false, _) => None,
            (true, Some(key)) => Some(key),
            (true, None) => return Err(ApiError::not_logged_in()),
        };
        #[cfg(test)]
        let url = match &self.base {
            Some(base) => format!("{base}{}", endpoints::path(endpoint, call.args)),
            None => endpoints::url(endpoint, call.args),
        };
        #[cfg(not(test))]
        let url = endpoints::url(endpoint, call.args);
        Ok(Request {
            method: endpoint.method,
            url,
            bearer,
            headers: call.headers,
            json: call.body,
            timeout,
        })
    }
}
