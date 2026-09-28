use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

const MODELS: &str = include_str!("../fixtures/api/models.json");

/// Keys the fake accepts, and what each stands for.
pub const ENV_KEY: &str = "test-key";
pub const DEVICE_KEY: &str = "device-key-9999";
pub const ACCOUNT_NUMBER: &str = "1234567890123456";

/// One scripted reply to a chat request.
pub enum Reply {
    /// An SSE fixture from tests/fixtures/sse/.
    Sse(&'static str),
    /// A JSON error envelope with this status and `error.type`.
    Error {
        status: u16,
        kind: &'static str,
        retry_after: Option<u64>,
    },
}

#[derive(Default)]
struct Inner {
    paths: Vec<String>,
    chat_bodies: Vec<Value>,
    script: VecDeque<Reply>,
    pair_pending: usize,
    pair_collected: bool,
}

type Shared = Arc<Mutex<Inner>>;

/// A local stand-in for crowbot: serves fixtures, follows a script, records what it was asked.
pub struct Fake {
    pub url: String,
    inner: Shared,
}

impl Fake {
    pub async fn start() -> Self {
        let inner: Shared = Arc::default();
        let app = Router::new()
            .route("/v1/models", get(models))
            .route("/v1/chat/completions", post(chat))
            .route("/api/me", get(me))
            .route("/api/pair/start", post(pair_start))
            .route("/api/pair/{code}", get(pair_poll))
            .layer(middleware::from_fn_with_state(inner.clone(), record))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, inner }
    }

    pub fn script(&self, replies: impl IntoIterator<Item = Reply>) {
        self.inner.lock().unwrap().script.extend(replies);
    }

    /// How many polls answer "pending" before the pairing is approved.
    pub fn pair_after(&self, pending: usize) {
        self.inner.lock().unwrap().pair_pending = pending;
    }

    pub fn hits(&self, path: &str) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.paths.iter().filter(|p| *p == path).count()
    }

    pub fn chat_bodies(&self) -> Vec<Value> {
        self.inner.lock().unwrap().chat_bodies.clone()
    }
}

async fn record(State(inner): State<Shared>, req: Request, next: Next) -> Response {
    inner
        .lock()
        .unwrap()
        .paths
        .push(req.uri().path().to_owned());
    next.run(req).await
}

fn authorized(headers: &HeaderMap) -> bool {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    matches!(bearer, Some(ENV_KEY | DEVICE_KEY | ACCOUNT_NUMBER))
}

fn error(status: u16, kind: &str, retry_after: Option<u64>) -> Response {
    let mut resp = (
        StatusCode::from_u16(status).unwrap(),
        [("x-request-id", "req_fake")],
        axum::Json(json!({"error": {"message": format!("fake {kind}"), "type": kind}})),
    )
        .into_response();
    if let Some(secs) = retry_after {
        resp.headers_mut()
            .insert(header::RETRY_AFTER, secs.to_string().parse().unwrap());
    }
    resp
}

async fn models() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], MODELS)
}

async fn chat(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    let reply = {
        let mut inner = inner.lock().unwrap();
        inner
            .chat_bodies
            .push(serde_json::from_str(&body).unwrap_or(Value::Null));
        inner.script.pop_front()
    };
    match reply {
        Some(Reply::Sse(name)) => {
            let path = format!("{}/tests/fixtures/sse/{name}", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{path}"));
            (
                [
                    (header::CONTENT_TYPE, "text/event-stream"),
                    (header::HeaderName::from_static("x-request-id"), "req_fake"),
                ],
                text,
            )
                .into_response()
        }
        Some(Reply::Error {
            status,
            kind,
            retry_after,
        }) => error(status, kind, retry_after),
        None => error(500, "unscripted", None),
    }
}

async fn me(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    axum::Json(json!({"balance_microdollars": 12_300_000, "can_spend": true})).into_response()
}

async fn pair_start() -> impl IntoResponse {
    axum::Json(json!({"device_code": "dev1", "user_code": "ABCD-1234", "ttl": 60}))
}

async fn pair_poll(State(inner): State<Shared>, Path(code): Path<String>) -> Response {
    let mut inner = inner.lock().unwrap();
    if code != "dev1" || inner.pair_collected {
        return error(410, "pair_expired", None);
    }
    if inner.pair_pending > 0 {
        inner.pair_pending -= 1;
        return (
            StatusCode::ACCEPTED,
            axum::Json(json!({"status": "pending"})),
        )
            .into_response();
    }
    inner.pair_collected = true;
    axum::Json(json!({"status": "ready", "api_key": DEVICE_KEY})).into_response()
}
