//! Tells pasted text from typing. Terminals with bracketed paste send a paste whole; on Windows
//! it arrives as a burst of key presses (`crowbot keytest`: 13 keys, no gap between them), and an
//! Enter inside that burst must add a line, not send the message.
//! CEILING: a timing heuristic; a very busy frame could make fast typing look like a paste.

use std::time::{Duration, Instant};

pub struct Burst {
    gap: Duration,
    min_keys: usize,
    last: Option<Instant>,
    run: usize,
}

impl Burst {
    pub fn new(gap: Duration, min_keys: usize) -> Self {
        Self {
            gap,
            min_keys,
            last: None,
            run: 0,
        }
    }

    /// Records a key press at `now`; true when it belongs to a paste, so Enter and Tab in it
    /// are text rather than commands.
    pub fn is_paste(&mut self, now: Instant) -> bool {
        let close = self
            .last
            .is_some_and(|last| now.duration_since(last) <= self.gap);
        self.run = if close { self.run + 1 } else { 1 };
        self.last = Some(now);
        self.run >= self.min_keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_becomes_a_paste_once_it_is_long_enough() {
        let start = crate::io::clock::instant();
        let mut burst = Burst::new(Duration::from_millis(5), 3);
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert!(!burst.is_paste(at(0)));
        assert!(!burst.is_paste(at(1)));
        assert!(burst.is_paste(at(2)));
        assert!(burst.is_paste(at(3)));
        // A human pause resets it: a typed Enter after typed text still sends.
        assert!(!burst.is_paste(at(200)));
    }

    #[test]
    fn typing_speed_is_never_a_paste() {
        let start = crate::io::clock::instant();
        let mut burst = Burst::new(Duration::from_millis(5), 3);
        for i in 0..20 {
            assert!(!burst.is_paste(start + Duration::from_millis(60 * i)));
        }
    }
}
