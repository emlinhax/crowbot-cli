//! Server-Sent Events as a stream of data events. Hand-rolled because bytes arrive split at
//! arbitrary points and crowbot's `:` keepalives must count as activity for the idle bound.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use futures_util::{Stream, StreamExt};

use crate::api::error::ApiError;
use crate::conversation::error::ErrorInfo;
use crate::io;
use crate::io::http::HttpError;
use crate::text::sse::Parser;

/// Each event's data from `body`, until `[DONE]` or the end. Silence longer than `idle`, no event
/// for `stall`, a failed read, and an event over `max_event_bytes` end the stream with an error.
pub fn events(
    body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
    idle: Duration,
    stall: Duration,
    max_event_bytes: usize,
) -> impl Stream<Item = Result<String, ErrorInfo>> {
    let state = State {
        body,
        parser: Parser::new(max_event_bytes),
        ready: VecDeque::new(),
        ended: false,
    };
    stream::unfold(state, move |mut st| async move {
        // Keepalives restart the idle bound but not this one: crowbot can keep sending them after
        // upstream has stopped.
        match io::clock::timeout(stall, next(&mut st, idle)).await {
            Ok(item) => item.map(|item| (item, st)),
            Err(_) => {
                st.ended = true;
                let error = ErrorInfo::local(
                    "idle_timeout",
                    format!("no reply for {}s, only keepalives", stall.as_secs()),
                );
                Some((Err(error), st))
            }
        }
    })
}

async fn next(st: &mut State, idle: Duration) -> Option<Result<String, ErrorInfo>> {
    loop {
        if let Some(data) = st.ready.pop_front() {
            return (data != "[DONE]").then_some(Ok(data));
        }
        if st.ended {
            return None;
        }
        // Any bytes, keepalives included, restart the idle bound.
        let failed = match io::clock::timeout(idle, st.body.next()).await {
            Err(_) => Some(ErrorInfo::local(
                "idle_timeout",
                format!("no data for {}s", idle.as_secs()),
            )),
            Ok(Some(Err(e))) => Some(ApiError::from(e).info),
            Ok(Some(Ok(bytes))) => match st.parser.feed(&bytes) {
                Ok(events) => {
                    st.ready.extend(events);
                    None
                }
                Err(long) => Some(ErrorInfo::local(
                    "bad_stream",
                    format!("an event ran past {} bytes", long.max),
                )),
            },
            Ok(None) => {
                st.ended = true;
                st.ready.extend(st.parser.finish());
                None
            }
        };
        if let Some(error) = failed {
            st.ended = true;
            return Some(Err(error));
        }
    }
}

struct State {
    body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
    parser: Parser,
    ready: VecDeque<String>,
    ended: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(chunks: &[&str]) -> BoxStream<'static, Result<Vec<u8>, HttpError>> {
        let chunks: Vec<_> = chunks.iter().map(|c| Ok(c.as_bytes().to_vec())).collect();
        stream::iter(chunks).boxed()
    }

    #[tokio::test]
    async fn events_end_at_done_or_with_the_reason_they_stopped() {
        let (idle, stall) = (Duration::from_secs(5), Duration::from_secs(5));
        let got: Vec<_> = events(
            body(&["data: a\n\ndata: [DONE]\n\ndata: b\n\n"]),
            idle,
            stall,
            64,
        )
        .collect()
        .await;
        assert_eq!(got, [Ok("a".to_owned())]);

        let silent = body(&["data: a\n\n"]).chain(stream::pending()).boxed();
        let got: Vec<_> = events(silent, Duration::from_millis(20), stall, 64)
            .collect()
            .await;
        assert_eq!(got[0], Ok("a".to_owned()));
        assert_eq!(got[1].as_ref().unwrap_err().kind, "idle_timeout");
        assert_eq!(got.len(), 2);
    }

    #[tokio::test]
    async fn keepalives_alone_stall_the_stream() {
        let keepalives = stream::unfold((), |()| async {
            io::clock::sleep(Duration::from_millis(5)).await;
            Some((Ok(b": keepalive\n\n".to_vec()), ()))
        });
        let body = body(&["data: a\n\n"]).chain(keepalives).boxed();
        let got: Vec<_> = events(body, Duration::from_secs(5), Duration::from_millis(60), 64)
            .collect()
            .await;
        assert_eq!(got[0], Ok("a".to_owned()));
        let error = got[1].as_ref().unwrap_err();
        assert_eq!(error.kind, "idle_timeout");
        assert!(
            error.message.contains("only keepalives"),
            "{}",
            error.message
        );
        assert_eq!(got.len(), 2);
    }
}
