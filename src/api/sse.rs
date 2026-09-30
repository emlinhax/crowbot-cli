//! Server-Sent Events as a stream of data events. Hand-rolled because bytes arrive split at
//! arbitrary points and crowbot's `:` keepalives must count as activity for the idle bound.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use futures_util::{Stream, StreamExt};

use crate::api::error::{ApiError, ErrorInfo};
use crate::io::http::HttpError;

/// Each event's data from `body`, until `[DONE]` or the end. Silence longer than `idle`, a failed
/// read, and an event over `max_event_bytes` end the stream with an error.
pub fn events(
    body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
    idle: Duration,
    max_event_bytes: usize,
) -> impl Stream<Item = Result<String, ErrorInfo>> {
    let state = State {
        body,
        parser: Parser::new(max_event_bytes),
        ready: VecDeque::new(),
        ended: false,
    };
    stream::unfold(state, move |mut st| async move {
        loop {
            if let Some(data) = st.ready.pop_front() {
                return (data != "[DONE]").then_some((Ok(data), st));
            }
            if st.ended {
                return None;
            }
            // Any bytes, keepalives included, restart the idle bound.
            let failed = match tokio::time::timeout(idle, st.body.next()).await {
                Err(_) => Some(ErrorInfo::local(
                    "idle_timeout",
                    format!("no data for {}s", idle.as_secs()),
                )),
                Ok(Some(Err(e))) => Some(ApiError::from(e).info),
                Ok(Some(Ok(bytes))) => st.parser.feed(&bytes).map(|e| st.ready.extend(e)).err(),
                Ok(None) => {
                    st.ended = true;
                    st.ready.extend(st.parser.finish());
                    None
                }
            };
            if let Some(error) = failed {
                st.ended = true;
                return Some((Err(error), st));
            }
        }
    })
}

struct State {
    body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
    parser: Parser,
    ready: VecDeque<String>,
    ended: bool,
}

struct Parser {
    pending: Vec<u8>,
    /// How much of `pending` is known to hold no newline, so a long line is scanned once.
    scanned: usize,
    data: Vec<String>,
    held: usize,
    max: usize,
}

impl Parser {
    fn new(max_event_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            scanned: 0,
            data: Vec::new(),
            held: 0,
            max: max_event_bytes,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, ErrorInfo> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(i) = self.pending[self.scanned..]
            .iter()
            .position(|&b| b == b'\n')
        {
            let end = self.scanned + i;
            let text = String::from_utf8_lossy(&self.pending[start..end]);
            self.held += line(&mut self.data, text.trim_end_matches('\r'), &mut events);
            if self.data.is_empty() {
                self.held = 0;
            }
            start = end + 1;
            self.scanned = start;
        }
        self.pending.drain(..start);
        self.scanned = self.pending.len();
        if self.pending.len() + self.held > self.max {
            return Err(ErrorInfo::local(
                "bad_stream",
                format!("an event ran past {} bytes", self.max),
            ));
        }
        Ok(events)
    }

    /// Flushes an event the stream ended without terminating.
    fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            line(
                &mut self.data,
                String::from_utf8_lossy(&rest).trim_end_matches('\r'),
                &mut events,
            );
        }
        line(&mut self.data, "", &mut events);
        events
    }
}

/// Takes one line into `data`, or on a blank one moves the finished event to `events`; returns
/// the bytes it added to `data`.
fn line(data: &mut Vec<String>, line: &str, events: &mut Vec<String>) -> usize {
    if line.is_empty() {
        if !data.is_empty() {
            events.push(data.join("\n"));
            data.clear();
        }
        return 0;
    }
    // Comments (`:` keepalives) and the other fields (event, id, retry) carry nothing crowbot uses.
    let Some(value) = line.strip_prefix("data:") else {
        return 0;
    };
    let value = value.strip_prefix(' ').unwrap_or(value);
    data.push(value.to_owned());
    value.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = ": keepalive\n\ndata: {\"a\":1}\r\n\r\ndata: line1\ndata: line2\n\nevent: x\ndata: [DONE]\n\n";

    fn parse_in_chunks(input: &[u8], sizes: &mut dyn Iterator<Item = usize>) -> Vec<String> {
        let mut parser = Parser::new(usize::MAX);
        let mut events = Vec::new();
        let mut rest = input;
        while !rest.is_empty() {
            let n = sizes.next().unwrap_or(1).clamp(1, rest.len());
            events.extend(parser.feed(&rest[..n]).unwrap());
            rest = &rest[n..];
        }
        events.extend(parser.finish());
        events
    }

    fn expected() -> Vec<String> {
        ["{\"a\":1}", "line1\nline2", "[DONE]"]
            .map(String::from)
            .to_vec()
    }

    fn body(chunks: &[&str]) -> BoxStream<'static, Result<Vec<u8>, HttpError>> {
        let chunks: Vec<_> = chunks.iter().map(|c| Ok(c.as_bytes().to_vec())).collect();
        stream::iter(chunks).boxed()
    }

    #[test]
    fn parses_whole_stream() {
        assert_eq!(
            parse_in_chunks(STREAM.as_bytes(), &mut std::iter::repeat(usize::MAX)),
            expected()
        );
    }

    #[test]
    fn any_chunking_gives_the_same_events() {
        let mut rng = fastrand::Rng::with_seed(7);
        for _ in 0..200 {
            let mut sizes = std::iter::from_fn(|| Some(rng.usize(1..8)));
            assert_eq!(parse_in_chunks(STREAM.as_bytes(), &mut sizes), expected());
        }
    }

    #[test]
    fn multibyte_characters_survive_splits() {
        let input = "data: héllo ✓\n\n".as_bytes();
        for split in 1..input.len() {
            let mut parser = Parser::new(usize::MAX);
            let mut events = parser.feed(&input[..split]).unwrap();
            events.extend(parser.feed(&input[split..]).unwrap());
            assert_eq!(events, ["héllo ✓"], "split at {split}");
        }
    }

    #[test]
    fn unterminated_event_is_flushed_at_end() {
        let mut parser = Parser::new(usize::MAX);
        assert!(parser.feed(b"data: tail").unwrap().is_empty());
        assert_eq!(parser.finish(), ["tail"]);
    }

    #[test]
    fn an_event_past_the_cap_is_a_broken_stream() {
        let mut parser = Parser::new(16);
        assert!(parser.feed(b"data: 0123456789").is_ok());
        let err = parser.feed(b"abcdefgh").unwrap_err();
        assert_eq!(err.kind, "bad_stream");
        let mut parser = Parser::new(16);
        assert!(parser.feed(b"data: 0123456789\n").is_ok());
        assert!(parser.feed(b"data: 0123456789\n").is_err());
        let mut parser = Parser::new(16);
        for _ in 0..4 {
            assert!(parser.feed(b"data: 0123456789\n\n").is_ok());
        }
    }

    #[tokio::test]
    async fn events_end_at_done_or_with_the_reason_they_stopped() {
        let idle = Duration::from_secs(5);
        let got: Vec<_> = events(body(&["data: a\n\ndata: [DONE]\n\ndata: b\n\n"]), idle, 64)
            .collect()
            .await;
        assert_eq!(got, [Ok("a".to_owned())]);

        let silent = body(&["data: a\n\n"]).chain(stream::pending()).boxed();
        let got: Vec<_> = events(silent, Duration::from_millis(20), 64)
            .collect()
            .await;
        assert_eq!(got[0], Ok("a".to_owned()));
        assert_eq!(got[1].as_ref().unwrap_err().kind, "idle_timeout");
        assert_eq!(got.len(), 2);
    }
}
