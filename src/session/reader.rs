//! Reading a session file back: the messages of every line that parses, in order. A line this
//! build cannot read (a newer build's type, part or finish) is skipped, not fatal.

use crate::agent::message::Message;
use crate::session::entry::Entry;

pub fn messages(text: &str) -> Vec<Message> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            Entry::Session { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_it_cannot_read_is_skipped() {
        let v1 = include_str!("../../tests/fixtures/session/v1.jsonl");
        let text = format!("{v1}{{\"type\":\"from_the_future\"}}\nnot json\n");
        assert_eq!(messages(&text).len(), messages(v1).len());
        assert_eq!(messages(v1).len(), 10);
    }
}
