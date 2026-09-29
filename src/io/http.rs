#![allow(clippy::disallowed_methods)]

use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
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
    pub bearer: Option<String>,
    pub headers: &'a [(&'a str, &'a str)],
    pub json: Option<&'a serde_json::Value>,
    /// Whole-request bound for `send`; for `open` it bounds only the wait for headers.
    pub timeout: Duration,
}

pub struct Response {
    pub status: u16,
    pub request_id: Option<String>,
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

/// A response whose body is still arriving.
pub struct Streaming {
    pub status: u16,
    pub request_id: Option<String>,
    pub retry_after: Option<Duration>,
    pub body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
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
        let resp = self
            .builder(&req)
            .timeout(req.timeout)
            .send()
            .await
            .map_err(classify)?;
        let head = Head::of(&resp);
        let body = resp.bytes().await.map_err(classify)?.to_vec();
        Ok(Response {
            status: head.status,
            request_id: head.request_id,
            retry_after: head.retry_after,
            body,
        })
    }

    /// Sends and returns as soon as headers arrive; the caller bounds each read of the body.
    pub async fn open(&self, req: Request<'_>) -> Result<Streaming, HttpError> {
        let resp = tokio::time::timeout(req.timeout, self.builder(&req).send())
            .await
            .map_err(|_| HttpError::Timeout)?
            .map_err(classify)?;
        let head = Head::of(&resp);
        Ok(Streaming {
            status: head.status,
            request_id: head.request_id,
            retry_after: head.retry_after,
            body: resp
                .bytes_stream()
                .map(|chunk| chunk.map(|b| b.to_vec()).map_err(classify))
                .boxed(),
        })
    }

    fn builder(&self, req: &Request<'_>) -> reqwest::RequestBuilder {
        let mut builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url),
        };
        if let Some(key) = &req.bearer {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in req.headers {
            builder = builder.header(*name, *value);
        }
        if let Some(body) = req.json {
            builder = builder.json(body);
        }
        builder
    }
}

struct Head {
    status: u16,
    request_id: Option<String>,
    retry_after: Option<Duration>,
}

impl Head {
    fn of(resp: &reqwest::Response) -> Self {
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            status: resp.status().as_u16(),
            request_id: header("x-request-id"),
            // CEILING: only the delta-seconds form; an HTTP-date Retry-After is ignored.
            retry_after: header("retry-after")
                .and_then(|v| v.trim().parse().ok())
                .map(Duration::from_secs),
        }
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
