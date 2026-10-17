//! A server reached at a URL over MCP's streamable HTTP: each message crowbot sends is a POST, and
//! the messages that answer it (one JSON body, or an SSE stream of them) come back as lines, so
//! `Rpc` talks to it as to a program's stdin and stdout.
// CEILING: no GET stream for messages a server starts on its own, no OAuth, no closing DELETE,
// and a server that forgets the session (a restart) is not introduced to again. Upgrade: each as
// a step of this carrier.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::config::limits;
use crate::io::http::{Http, Method, Request, Streaming};
use crate::mcp::protocol::DATA;
use crate::text::sse::Parser;
use crate::text::template::fill;

/// The lines a server sends and the way to send it lines, as a started program has them.
pub struct Opened {
    pub incoming: mpsc::UnboundedReceiver<Result<String, String>>,
    pub outgoing: mpsc::UnboundedSender<String>,
    pub connection: Connection,
}

/// The task carrying crowbot's messages; dropping it drops every exchange still going.
pub struct Connection(tokio::task::JoinHandle<()>);

impl Drop for Connection {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What every exchange with one server shares.
struct Link {
    http: Http,
    url: String,
    headers: BTreeMap<String, String>,
    /// How long an answer may take to start, and to arrive whole.
    limit: Duration,
    /// The session the server opened with its answer to `initialize`.
    session: Mutex<Option<String>>,
    /// The protocol version it answered with, sent on every request after.
    version: Mutex<Option<String>>,
}

/// Starts carrying lines to the server at `url`, each POSTed on its own, so a slow call holds up
/// no other message (a cancel, a second call).
pub fn open(http: &Http, url: &str, headers: &BTreeMap<String, String>, limit: Duration) -> Opened {
    let (outgoing, mut sent) = mpsc::unbounded_channel::<String>();
    let (answers, incoming) = mpsc::unbounded_channel();
    let link = Arc::new(Link {
        http: http.clone(),
        url: url.to_owned(),
        headers: headers.clone(),
        limit,
        session: Mutex::new(None),
        version: Mutex::new(None),
    });
    let carrier = tokio::spawn(async move {
        let mut exchanges = JoinSet::new();
        loop {
            tokio::select! {
                line = sent.recv() => match line {
                    Some(line) => {
                        exchanges.spawn(exchange(link.clone(), line, answers.clone()));
                    }
                    None => break,
                },
                Some(_) = exchanges.join_next(), if !exchanges.is_empty() => {}
            }
        }
    });
    Opened {
        incoming,
        outgoing,
        connection: Connection(carrier),
    }
}

/// POSTs one message and passes on what answers it. A request whose POST fails is answered with
/// a JSON-RPC error saying why, so only that call fails; a notification's failure goes unsaid.
async fn exchange(
    link: Arc<Link>,
    line: String,
    answers: mpsc::UnboundedSender<Result<String, String>>,
) {
    let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    if let Err(why) = post(&link, &message, &answers).await
        && let Some(id) = message.get("id")
    {
        let failed = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": why}});
        let _ = answers.send(Ok(failed.to_string()));
    }
}

async fn post(
    link: &Link,
    message: &Value,
    answers: &mpsc::UnboundedSender<Result<String, String>>,
) -> Result<(), String> {
    let session = lock(&link.session).clone();
    let version = lock(&link.version).clone();
    let mut headers = vec![("Accept", "application/json, text/event-stream")];
    if let Some(session) = &session {
        headers.push(("Mcp-Session-Id", session));
    }
    if let Some(version) = &version {
        headers.push(("MCP-Protocol-Version", version));
    }
    headers.extend(link.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let answer = link
        .http
        .open(Request {
            method: Method::Post,
            url: link.url.clone(),
            bearer: None,
            headers: &headers,
            json: Some(message),
            timeout: link.limit,
        })
        .await
        .map_err(|e| e.to_string())?;
    if let Some(given) = answer.head.header("mcp-session-id") {
        *lock(&link.session) = Some(given.to_owned());
    }
    match answer.head.status {
        // A notification or an answer taken: nothing comes back.
        202 => Ok(()),
        200..=299 => read(link, answer, answers).await,
        status => Err(fill(
            &DATA.text.http_status,
            &[("status", &status.to_string())],
        )),
    }
}

/// Passes on each message of an answer: an SSE stream event by event as they arrive, or a JSON
/// body (one message, or a batch of them).
async fn read(
    link: &Link,
    mut answer: Streaming,
    answers: &mpsc::UnboundedSender<Result<String, String>>,
) -> Result<(), String> {
    let most = limits::get().mcp.line_bytes.value;
    let pass = |message: &str| {
        note_version(link, message);
        let _ = answers.send(Ok(message.to_owned()));
    };
    let streamed = answer
        .head
        .header("content-type")
        .is_some_and(|t| t.starts_with("text/event-stream"));
    if !streamed {
        let body = answer.collect(most, link.limit).await.body;
        let text = String::from_utf8_lossy(&body);
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(batch)) => batch.iter().for_each(|m| pass(&m.to_string())),
            _ => pass(text.trim()),
        }
        return Ok(());
    }
    let mut parser = Parser::new(most);
    while let Some(chunk) = answer.body.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        let events = parser
            .feed(&chunk)
            .map_err(|long| fill(&DATA.text.too_long, &[("bytes", &long.max.to_string())]))?;
        events.iter().for_each(|event| pass(event));
    }
    parser.finish().iter().for_each(|event| pass(event));
    Ok(())
}

/// Keeps the protocol version the server answered `initialize` with, for the requests after.
fn note_version(link: &Link, message: &str) {
    if lock(&link.version).is_some() {
        return;
    }
    let version = serde_json::from_str::<Value>(message)
        .ok()
        .and_then(|m| m["result"]["protocolVersion"].as_str().map(str::to_owned));
    if version.is_some() {
        *lock(&link.version) = version;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use serde_json::{Value, json};

    use crate::config::paths::Paths;
    use crate::io::http::Http;
    use crate::mcp::config::{Reach, Server as Config};
    use crate::mcp::server::Server;

    /// A server over streamable HTTP: it opens session `s1` at `initialize` and refuses any later
    /// request without it or without the version it answered; it answers in SSE when `streamed`.
    fn answer(streamed: bool, headers: &HeaderMap, body: &Value) -> Response {
        let method = body["method"].as_str().unwrap_or_default();
        let id = body.get("id").cloned();
        if method != "initialize" {
            let seen = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
            if seen("mcp-session-id") != Some("s1") || seen("mcp-protocol-version").is_none() {
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
        let Some(id) = id else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match method {
            "initialize" => json!({
                "protocolVersion": body["params"]["protocolVersion"],
                "capabilities": {"tools": {}}
            }),
            "tools/list" => json!({"tools": [{"name": "echo", "inputSchema": {"type": "object"}}]}),
            "tools/call" => {
                let said = &body["params"]["arguments"]["text"];
                json!({"content": [{"type": "text", "text": said}]})
            }
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        let message = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        let mut response = if streamed {
            let events = format!(": opened\n\nevent: message\ndata: {message}\n\n");
            ([(header::CONTENT_TYPE, "text/event-stream")], events).into_response()
        } else {
            ([(header::CONTENT_TYPE, "application/json")], message).into_response()
        };
        if method == "initialize" {
            response
                .headers_mut()
                .insert("mcp-session-id", "s1".parse().unwrap());
        }
        response
    }

    async fn stand_in() -> String {
        crate::testing::serve(|_| {
            Router::new()
                .route(
                    "/json",
                    post(|h: HeaderMap, b: axum::Json<Value>| async move { answer(false, &h, &b) }),
                )
                .route(
                    "/sse",
                    post(|h: HeaderMap, b: axum::Json<Value>| async move { answer(true, &h, &b) }),
                )
                .route(
                    "/broken",
                    post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
                )
        })
        .await
    }

    async fn start(url: String) -> Result<Server, String> {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().to_path_buf());
        let config = Config {
            reach: Reach::Url {
                url,
                headers: BTreeMap::new(),
            },
            enabled: true,
            timeout_secs: Some(5),
            tools: None,
        };
        let web = Http::new(std::time::Duration::from_secs(2)).unwrap();
        Server::start("web", &config, &paths, &web).await
    }

    #[tokio::test]
    async fn a_server_at_a_url_is_introduced_listed_and_called_in_json_or_sse() {
        let base = stand_in().await;
        for route in ["json", "sse"] {
            let server = start(format!("{base}/{route}")).await.unwrap();
            let names: Vec<&str> = server.tools.iter().map(|t| t.name.as_str()).collect();
            assert_eq!(names, ["echo"], "{route}");
            let done = server
                .caller()
                .call("echo", json!({"text": "hi"}))
                .await
                .unwrap();
            assert_eq!(done.content[0]["text"], "hi", "{route}");
            server.stop().await;
        }
    }

    #[tokio::test]
    async fn a_url_that_answers_an_error_or_nothing_is_not_running_and_says_why() {
        let base = stand_in().await;
        let broken = start(format!("{base}/broken")).await.err().unwrap();
        assert!(broken.contains("HTTP 500"), "{broken}");
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let gone = start(format!("http://127.0.0.1:{closed}/mcp"))
            .await
            .err()
            .unwrap();
        assert!(gone.contains("connect"), "{gone}");
    }
}
