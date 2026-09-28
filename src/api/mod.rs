//! crowbot's HTTP surface: endpoint catalog, typed errors, and one call path for every request.

pub mod endpoints;
pub mod error;
pub mod models;

use std::time::Duration;

use crate::io::http::{Http, Request, Response};
use error::ApiError;

pub struct Api {
    http: Http,
}

impl Api {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// Calls the endpoint named `id` in data/endpoints.toml; `args` fill its `{placeholders}`.
    pub async fn call(
        &self,
        id: &str,
        args: &[(&str, &str)],
        body: Option<&serde_json::Value>,
        timeout: Duration,
    ) -> Result<Response, ApiError> {
        let endpoint = endpoints::get(id);
        let resp = self
            .http
            .send(Request {
                method: endpoint.method,
                url: endpoints::url(endpoint, args),
                bearer: None,
                json: body,
                timeout,
            })
            .await?;
        if resp.status >= 400 {
            return Err(ApiError::from_response(&resp));
        }
        Ok(resp)
    }
}
