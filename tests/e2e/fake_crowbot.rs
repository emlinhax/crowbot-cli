use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use serde_json::{Value, json};

const MODELS: &str = include_str!("../fixtures/api/models.json");

/// Keys the fake accepts, and what each stands for.
pub const ENV_KEY: &str = "test-key";
pub const DEVICE_KEY: &str = "device-key-9999";
pub const ACCOUNT_NUMBER: &str = "1234567890123456";

/// One scripted reply to a chat request.
pub enum Reply {
    /// An SSE fixture from tests/fixtures/sse/.
    Sse(String),
    /// A JSON error envelope with this status and `error.type`.
    Error {
        status: u16,
        kind: &'static str,
        retry_after: Option<u64>,
    },
    /// An error that sends its headers (retry at once) and the start of its body, then nothing.
    Stall { status: u16 },
}

#[derive(Default)]
struct Inner {
    /// The fake's own address, filled into scripted replies as `{base}` so a scripted model
    /// can point webfetch at this server.
    base: String,
    paths: Vec<String>,
    chat_bodies: Vec<Value>,
    script: VecDeque<Reply>,
    pair_pending: usize,
    pair_collected: bool,
    /// Signups to refuse with 428 before accepting one.
    stale_proofs: usize,
}

type Shared = Arc<Mutex<Inner>>;

/// A local stand-in for crowbot: serves fixtures, follows a script, records what it was asked.
pub struct Fake {
    pub url: String,
    inner: Shared,
}

impl Reply {
    pub fn sse(name: &str) -> Self {
        Self::Sse(name.to_owned())
    }
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
            .route("/api/pow", get(pow))
            .route("/api/signup", post(signup))
            .route("/web/ok", get(web_ok))
            .route("/web/blocked", get(web_blocked))
            .layer(middleware::from_fn_with_state(inner.clone(), record))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        inner.lock().unwrap().base = url.clone();
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

    pub fn stale_proofs(&self, count: usize) {
        self.inner.lock().unwrap().stale_proofs = count;
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

/// A plain page, as a documentation site serves it.
async fn web_ok() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<html><head><title>Widget docs</title></head><body>\
         <h1>Widget API</h1><p>The widget spins at 3 rpm.</p></body></html>",
    )
}

/// What Cloudflare serves while it challenges a client: 403, its mitigation header, and an
/// interstitial that must never reach the model as if it were the page.
async fn web_blocked() -> Response {
    (
        StatusCode::FORBIDDEN,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::HeaderName::from_static("cf-mitigated"), "challenge"),
        ],
        "<html><head><title>Just a moment...</title></head><body>\
         <script>window._cf_chl_opt={cType:'managed'};</script>\
         <noscript>Enable JavaScript and cookies to continue</noscript></body></html>",
    )
        .into_response()
}

async fn models() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], MODELS)
}

async fn chat(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    let (reply, base) = {
        let mut inner = inner.lock().unwrap();
        inner
            .chat_bodies
            .push(serde_json::from_str(&body).unwrap_or(Value::Null));
        (inner.script.pop_front(), inner.base.clone())
    };
    match reply {
        Some(Reply::Sse(name)) => {
            let path = format!("{}/tests/fixtures/sse/{name}", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("{path}"))
                .replace("{base}", &base);
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
        Some(Reply::Stall { status }) => {
            let start =
                futures_util::stream::once(async { Ok::<_, std::io::Error>(r#"{"error":"#) });
            let body = axum::body::Body::from_stream(start.chain(futures_util::stream::pending()));
            (
                StatusCode::from_u16(status).unwrap(),
                [("x-request-id", "req_fake"), ("retry-after", "0")],
                body,
            )
                .into_response()
        }
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

const POW_BITS: u32 = 8;

async fn pow() -> impl IntoResponse {
    axum::Json(json!({"challenge": "fakechallenge", "bits": POW_BITS, "ttl": 60}))
}

async fn signup(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    use sha2::{Digest, Sha256};
    let proof = headers
        .get("x-pow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some((challenge, nonce)) = proof.split_once('.') else {
        return error(428, "pow_required", None);
    };
    let hash = Sha256::digest(format!("{challenge}:{nonce}"));
    let zeros = hash
        .iter()
        .scan(true, |counting, b| {
            let bits = if *counting { b.leading_zeros() } else { 0 };
            *counting &= *b == 0;
            Some(bits)
        })
        .sum::<u32>();
    let mut inner = inner.lock().unwrap();
    if challenge != "fakechallenge" || zeros < POW_BITS || inner.stale_proofs > 0 {
        inner.stale_proofs = inner.stale_proofs.saturating_sub(1);
        return error(428, "pow_required", None);
    }
    axum::Json(json!({
        "account_number": ACCOUNT_NUMBER,
        "formatted": "1234 5678 9012 3456",
        "api_base": "https://api.crowbot.sh/v1"
    }))
    .into_response()
}
