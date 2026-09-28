#![allow(clippy::disallowed_methods)]

use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
}

pub struct Request<'a> {
    pub method: Method,
    pub url: String,
    pub bearer: Option<&'a str>,
    pub json: Option<&'a serde_json::Value>,
    pub timeout: Duration,
}

pub struct Response {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("timed out")]
    Timeout,
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new(connect_timeout: Duration) -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .user_agent(concat!("crowbot/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HttpError::Other(e.to_string()))?;
        Ok(Self { client })
    }

    pub async fn send(&self, req: Request<'_>) -> Result<Response, HttpError> {
        let mut builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url),
        }
        .timeout(req.timeout);
        if let Some(key) = req.bearer {
            builder = builder.bearer_auth(key);
        }
        if let Some(body) = req.json {
            builder = builder.json(body);
        }
        let resp = builder.send().await.map_err(classify)?;
        let status = resp.status().as_u16();
        let request_id = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = resp.bytes().await.map_err(classify)?.to_vec();
        Ok(Response {
            status,
            request_id,
            body,
        })
    }
}

fn classify(e: reqwest::Error) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout
    } else if e.is_connect() {
        HttpError::Connect(e.to_string())
    } else {
        HttpError::Other(e.to_string())
    }
}
