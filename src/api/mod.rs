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
    key: Arc<RwLock<Key>>,
    /// Replaces every origin; lets unit tests talk to a local fake.
    #[cfg(test)]
    base: Option<String>,
}

#[derive(Default)]
struct Key {
    key: Option<String>,
    /// Bumped on every `set_key`, so a session can tell its account changed without being told.
    changes: u64,
}

impl Key {
    fn new(key: Option<String>) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self { key, changes: 0 }))
    }
}

/// One call: the endpoint id from data/endpoints.toml and what fills it in.
#[derive(Default)]
pub struct Call<'a> {
    /// Values for the endpoint path's `{placeholders}`.
    pub args: &'a [(&'a str, &'a str)],
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a serde_json::Value>,
    /// How long the reply may take; `None` is http.request_timeout_ms (data/limits.toml).
    pub timeout: Option<Duration>,
}

impl Api {
    pub fn new(http: Http, key: Option<String>) -> Self {
        Self {
            http,
            key: Key::new(key),
            #[cfg(test)]
            base: None,
        }
    }

    pub fn has_key(&self) -> bool {
        self.key().is_some()
    }

    /// Replaces the key for this client and every clone of it.
    pub fn set_key(&self, key: Option<String>) {
        let mut held = self.key.write().unwrap_or_else(|e| e.into_inner());
        held.key = key;
        held.changes += 1;
    }

    /// How many times the key has been replaced; a change means another account, or none.
    pub fn key_changes(&self) -> u64 {
        self.key.read().unwrap_or_else(|e| e.into_inner()).changes
    }

    /// A separate client acting as another key, e.g. to check a key before saving it.
    pub fn with_key(&self, key: String) -> Self {
        Self {
            key: Key::new(Some(key)),
            ..self.clone()
        }
    }

    fn key(&self) -> Option<String> {
        self.key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .key
            .clone()
    }

    #[cfg(test)]
    pub fn redirected(http: Http, key: &str, base: &str) -> Self {
        Self {
            http,
            key: Key::new(Some(key.to_owned())),
            base: Some(base.to_owned()),
        }
    }

    pub async fn call(&self, id: &str, call: Call<'_>) -> Result<Response, ApiError> {
        let resp = self.http.send(self.request(id, &call)?).await?;
        if resp.head.status >= 400 {
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }

    /// `call`, with the reply read as JSON.
    pub async fn call_json<T: serde::de::DeserializeOwned>(
        &self,
        id: &str,
        call: Call<'_>,
    ) -> Result<T, ApiError> {
        let resp = self.call(id, call).await?;
        error::json(&resp.body)
    }

    /// Like `call`, but streams the body. An error's body is read within `http.error_body_*`.
    pub async fn open(&self, id: &str, call: Call<'_>) -> Result<Streaming, ApiError> {
        let resp = self.http.open(self.request(id, &call)?).await?;
        if resp.head.status >= 400 {
            let limits = &crate::config::limits::get().http;
            let resp = resp
                .collect(limits.error_body_bytes.value, limits.error_body_ms.ms())
                .await;
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }

    fn request<'a>(&'a self, id: &str, call: &Call<'a>) -> Result<Request<'a>, ApiError> {
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
            timeout: call
                .timeout
                .unwrap_or_else(|| crate::config::limits::get().http.request_timeout_ms.ms()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clone_sees_a_key_change_and_a_separate_client_does_not() {
        let api = Api::new(Http::new(Duration::from_secs(1)).unwrap(), None);
        let clone = api.clone();
        let other = api.with_key("k".into());
        api.set_key(Some("k".into()));
        assert_eq!((clone.key_changes(), clone.has_key()), (1, true));
        api.set_key(None);
        assert_eq!(clone.key_changes(), 2);
        assert_eq!(other.key_changes(), 0);
    }
}
