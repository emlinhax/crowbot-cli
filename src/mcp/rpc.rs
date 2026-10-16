//! JSON-RPC 2.0 over a pair of line channels, one message a line: requests matched to their
//! answers by id, and the server's own few requests answered. What fills the channels (a
//! program's stdin and stdout) is not this file's concern.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::io::clock;

#[derive(Debug, PartialEq)]
pub enum Error {
    /// The server stopped answering: why, as best known.
    Closed(String),
    TimedOut,
    /// The server answered with an error.
    Remote(String),
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Error>>>>>;

pub struct Rpc {
    outgoing: mpsc::UnboundedSender<String>,
    waiting: Waiting,
    next: AtomicU64,
    /// Why the server stopped, once it has.
    closed: Arc<Mutex<Option<String>>>,
}

impl Rpc {
    /// Talks over `outgoing` and `incoming`; `gone` says why when `incoming` ends with no reason.
    pub fn new(
        outgoing: mpsc::UnboundedSender<String>,
        incoming: mpsc::UnboundedReceiver<Result<String, String>>,
        gone: String,
    ) -> Self {
        let waiting = Waiting::default();
        let closed = Arc::new(Mutex::new(None));
        tokio::spawn(dispatch(
            incoming,
            outgoing.clone(),
            waiting.clone(),
            closed.clone(),
            gone,
        ));
        Self {
            outgoing,
            waiting,
            next: AtomicU64::new(1),
            closed,
        }
    }

    /// Why the server stopped, if it has.
    pub fn closed(&self) -> Option<String> {
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Asks `method` and waits up to `limit` for the answer.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, Error> {
        if let Some(why) = self.closed() {
            return Err(Error::Closed(why));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.waiting).insert(id, tx);
        // Whatever ends the wait (an answer, a timeout, the caller giving up) forgets the id.
        let _forget = Forget {
            waiting: &self.waiting,
            id,
        };
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.outgoing.send(line.to_string()).is_err() {
            return Err(Error::Closed(self.closed().unwrap_or_default()));
        }
        match clock::timeout(limit, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(Error::Closed(self.closed().unwrap_or_default())),
            Err(_) => Err(Error::TimedOut),
        }
    }

    /// Tells the server `method`, which it does not answer.
    pub fn notify(&self, method: &str, params: Value) {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let _ = self.outgoing.send(line.to_string());
    }
}

struct Forget<'a> {
    waiting: &'a Waiting,
    id: u64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        lock(self.waiting).remove(&self.id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

async fn dispatch(
    mut incoming: mpsc::UnboundedReceiver<Result<String, String>>,
    outgoing: mpsc::UnboundedSender<String>,
    waiting: Waiting,
    closed: Arc<Mutex<Option<String>>>,
    gone: String,
) {
    let why = loop {
        let line = match incoming.recv().await {
            Some(Ok(line)) => line,
            Some(Err(why)) => break why,
            None => break gone,
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned();
        match (message.get("method").and_then(Value::as_str), id) {
            // The server asks something of crowbot, which offers nothing but a ping's answer.
            (Some(method), Some(id)) => {
                let answer = match method {
                    "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
                    _ => json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "crowbot offers no such method"}}),
                };
                let _ = outgoing.send(answer.to_string());
            }
            // CEILING: notifications are ignored, `notifications/tools/list_changed` among them:
            // a server's tools are the ones it listed at the start. Upgrade: list them again and
            // offer the new list from the next run on.
            (Some(_), None) => {}
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else { continue };
                let Some(tx) = lock(&waiting).remove(&id) else {
                    continue;
                };
                let answer = match message.get("error") {
                    Some(error) => Err(Error::Remote(
                        error["message"].as_str().unwrap_or("an error").to_owned(),
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(answer);
            }
            (None, None) => {}
        }
    };
    *lock(&closed) = Some(why.clone());
    for (_, tx) in lock(&waiting).drain() {
        let _ = tx.send(Err(Error::Closed(why.clone())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `Rpc` and the other ends of its channels.
    fn pair() -> (
        Rpc,
        mpsc::UnboundedReceiver<String>,
        mpsc::UnboundedSender<Result<String, String>>,
    ) {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        (Rpc::new(out_tx, in_rx, "it exited".into()), out_rx, in_tx)
    }

    fn sent(line: Option<String>) -> Value {
        serde_json::from_str(&line.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn answers_are_matched_to_requests_by_id_in_any_order() {
        let (rpc, mut out, answers) = pair();
        let rpc = Arc::new(rpc);
        let (a, b) = (rpc.clone(), rpc.clone());
        let first =
            tokio::spawn(async move { a.request("a", json!({}), Duration::from_secs(5)).await });
        let one = sent(out.recv().await);
        let second =
            tokio::spawn(async move { b.request("b", json!({}), Duration::from_secs(5)).await });
        let two = sent(out.recv().await);
        for (message, said) in [(&two, "second"), (&one, "first")] {
            let id = &message["id"];
            answers
                .send(Ok(
                    json!({"jsonrpc": "2.0", "id": id, "result": said}).to_string()
                ))
                .unwrap();
        }
        assert_eq!(first.await.unwrap(), Ok(json!("first")));
        assert_eq!(second.await.unwrap(), Ok(json!("second")));
    }

    #[tokio::test]
    async fn a_ping_is_answered_and_other_requests_refused() {
        let (_rpc, mut out, answers) = pair();
        answers
            .send(Ok(
                json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}).to_string()
            ))
            .unwrap();
        assert_eq!(sent(out.recv().await)["result"], json!({}));
        answers
            .send(Ok(
                json!({"jsonrpc": "2.0", "id": 8, "method": "sampling/createMessage"}).to_string(),
            ))
            .unwrap();
        assert_eq!(sent(out.recv().await)["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_server_that_stops_fails_what_waits_and_what_comes_after() {
        let (rpc, mut out, answers) = pair();
        let rpc = Arc::new(rpc);
        let waiting = rpc.clone();
        let pending = tokio::spawn(async move {
            waiting
                .request("x", json!({}), Duration::from_secs(5))
                .await
        });
        let _ = out.recv().await;
        drop(answers);
        assert_eq!(
            pending.await.unwrap(),
            Err(Error::Closed("it exited".into()))
        );
        let later = rpc.request("y", json!({}), Duration::from_secs(5)).await;
        assert_eq!(later, Err(Error::Closed("it exited".into())));
    }

    #[tokio::test]
    async fn a_request_past_its_limit_times_out_and_is_forgotten() {
        let (rpc, _out, _answers) = pair();
        let done = rpc
            .request("slow", json!({}), Duration::from_millis(20))
            .await;
        assert_eq!(done, Err(Error::TimedOut));
        assert!(lock(&rpc.waiting).is_empty());
    }
}
