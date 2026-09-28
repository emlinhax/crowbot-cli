//! crowbot's HTTP surface: endpoint catalog, typed errors, and one call path for every request.

pub mod account;
pub mod assemble;
pub mod chat;
pub mod endpoints;
pub mod error;
pub mod models;
pub mod pair;
pub mod retry;
pub mod sse;
pub mod wire;

use std::time::Duration;

use crate::io::http::{Http, Request, Response, Streaming};
use error::ApiError;

pub struct Api {
    http: Http,
    key: Option<String>,
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
        Self { http, key }
    }

    /// The same client acting as another key, e.g. to check a key before saving it.
    pub fn with_key(&self, key: String) -> Self {
        Self {
            http: self.http.clone(),
            key: Some(key),
        }
    }

    pub async fn call(
        &self,
        id: &str,
        call: Call<'_>,
        timeout: Duration,
    ) -> Result<Response, ApiError> {
        let resp = self.http.send(self.request(id, &call, timeout)?).await?;
        if resp.status >= 400 {
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }

    /// Like `call`, but hands back the body as it streams; errors before the body are read whole.
    pub async fn open(
        &self,
        id: &str,
        call: Call<'_>,
        timeout: Duration,
    ) -> Result<Streaming, ApiError> {
        let mut resp = self.http.open(self.request(id, &call, timeout)?).await?;
        if resp.status >= 400 {
            use futures_util::StreamExt;
            let mut body = Vec::new();
            while let Some(Ok(chunk)) = resp.body.next().await {
                body.extend(chunk);
            }
            return Err(ApiError::from_response(&Response {
                status: resp.status,
                request_id: resp.request_id,
                retry_after: resp.retry_after,
                body,
            }));
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
        let bearer = match (endpoint.auth, &self.key) {
            (false, _) => None,
            (true, Some(key)) => Some(key.as_str()),
            (true, None) => return Err(ApiError::not_logged_in()),
        };
        Ok(Request {
            method: endpoint.method,
            url: endpoints::url(endpoint, call.args),
            bearer,
            headers: call.headers,
            json: call.body,
            timeout,
        })
    }
}
