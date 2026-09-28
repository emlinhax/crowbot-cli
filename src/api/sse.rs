//! Incremental Server-Sent Events parser. Hand-rolled because crowbot's `:` keepalives must be
//! visible (they reset the idle timer) and bytes arrive split at arbitrary points.

#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Data(String),
    /// A `:` comment line; crowbot sends these as keepalives during pauses.
    Comment,
}

#[derive(Default)]
pub struct Parser {
    pending: Vec<u8>,
    data: Vec<String>,
}

impl Parser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.pending.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            self.line(line.trim_end_matches(['\n', '\r']), &mut frames);
        }
        frames
    }

    /// Flushes an event the stream ended without terminating.
    pub fn finish(&mut self) -> Vec<Frame> {
        let mut frames = Vec::new();
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let line = String::from_utf8_lossy(&rest);
            self.line(line.trim_end_matches('\r'), &mut frames);
        }
        self.dispatch(&mut frames);
        frames
    }

    fn line(&mut self, line: &str, frames: &mut Vec<Frame>) {
        if line.is_empty() {
            self.dispatch(frames);
        } else if line.starts_with(':') {
            frames.push(Frame::Comment);
        } else if let Some(value) = line.strip_prefix("data:") {
            self.data
                .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
        }
        // Other fields (event, id, retry) carry nothing crowbot uses.
    }

    fn dispatch(&mut self, frames: &mut Vec<Frame>) {
        if !self.data.is_empty() {
            frames.push(Frame::Data(self.data.join("\n")));
            self.data.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = ": keepalive\n\ndata: {\"a\":1}\r\n\r\ndata: line1\ndata: line2\n\nevent: x\ndata: [DONE]\n\n";

    fn parse_in_chunks(input: &[u8], sizes: &mut dyn Iterator<Item = usize>) -> Vec<Frame> {
        let mut parser = Parser::default();
        let mut frames = Vec::new();
        let mut rest = input;
        while !rest.is_empty() {
            let n = sizes.next().unwrap_or(1).clamp(1, rest.len());
            frames.extend(parser.feed(&rest[..n]));
            rest = &rest[n..];
        }
        frames.extend(parser.finish());
        frames
    }

    fn expected() -> Vec<Frame> {
        vec![
            Frame::Comment,
            Frame::Data("{\"a\":1}".into()),
            Frame::Data("line1\nline2".into()),
            Frame::Data("[DONE]".into()),
        ]
    }

    #[test]
    fn parses_whole_stream() {
        assert_eq!(
            parse_in_chunks(STREAM.as_bytes(), &mut std::iter::repeat(usize::MAX)),
            expected()
        );
    }

    #[test]
    fn any_chunking_gives_the_same_frames() {
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
            let mut parser = Parser::default();
            let mut frames = parser.feed(&input[..split]);
            frames.extend(parser.feed(&input[split..]));
            assert_eq!(
                frames,
                vec![Frame::Data("héllo ✓".into())],
                "split at {split}"
            );
        }
    }

    #[test]
    fn unterminated_event_is_flushed_at_end() {
        let mut parser = Parser::default();
        assert!(parser.feed(b"data: tail").is_empty());
        assert_eq!(parser.finish(), vec![Frame::Data("tail".into())]);
    }
}
