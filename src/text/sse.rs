//! Server-Sent Events read off bytes that arrive split at any point: each event's `data:` lines,
//! joined. What a stream means (crowbot's `[DONE]`, an MCP server's JSON-RPC) is its reader's.

/// An event ran past `max` bytes: the stream is broken.
#[derive(Debug)]
pub struct TooLong {
    pub max: usize,
}

pub struct Parser {
    pending: Vec<u8>,
    /// How much of `pending` is known to hold no newline, so a long line is scanned once.
    scanned: usize,
    data: Vec<String>,
    held: usize,
    max: usize,
}

impl Parser {
    pub fn new(max_event_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            scanned: 0,
            data: Vec::new(),
            held: 0,
            max: max_event_bytes,
        }
    }

    /// The events `bytes` completes, in order; `Err` once one runs past the cap.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, TooLong> {
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
            return Err(TooLong { max: self.max });
        }
        Ok(events)
    }

    /// Flushes an event the stream ended without terminating.
    pub fn finish(&mut self) -> Vec<String> {
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
        assert_eq!(err.max, 16);
        let mut parser = Parser::new(16);
        assert!(parser.feed(b"data: 0123456789\n").is_ok());
        assert!(parser.feed(b"data: 0123456789\n").is_err());
        let mut parser = Parser::new(16);
        for _ in 0..4 {
            assert!(parser.feed(b"data: 0123456789\n\n").is_ok());
        }
    }
}
