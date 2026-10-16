//! Reading a session file back: the current branch of its conversation. A line this build cannot
//! read (a newer build's type, part or finish) is skipped, not fatal.
//!
//! The head runs through the file in order. A message follows its `parent` (the nearest earlier
//! entry with that id) or, without a known one, the head where it was written; then it becomes
//! the head. A rewind moves the head back. The branch is the chain from the last head to its
//! root, so older files without parents read as one straight line.

use std::collections::HashMap;

use crate::conversation::message::Message;
use crate::session::entry::Entry;

/// The conversation on the current branch, each message with its entry id.
pub struct Loaded {
    pub branch: Vec<(String, Message)>,
}

struct Node {
    id: String,
    parent: Option<usize>,
    message: Message,
}

pub fn read(text: &str) -> Loaded {
    let mut nodes: Vec<Node> = Vec::new();
    // The latest node with each id, which is what a later line naming it means.
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut head: Option<usize> = None;
    for entry in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
    {
        match entry {
            Entry::Message {
                id,
                parent,
                message,
                ..
            } => {
                let parent = parent.and_then(|p| by_id.get(&p).copied()).or(head);
                by_id.insert(id.clone(), nodes.len());
                head = Some(nodes.len());
                nodes.push(Node {
                    id,
                    parent,
                    message,
                });
            }
            Entry::Rewind { to: None, .. } => head = None,
            // A rewind to an id this file never had is ignored.
            Entry::Rewind { to: Some(to), .. } => {
                if let Some(&i) = by_id.get(&to) {
                    head = Some(i);
                }
            }
            Entry::Session { .. } => {}
        }
    }
    let mut chain = Vec::new();
    let mut at = head;
    while let Some(i) = at {
        chain.push(i);
        at = nodes[i].parent;
    }
    let mut nodes: Vec<Option<Node>> = nodes.into_iter().map(Some).collect();
    let branch = chain
        .into_iter()
        .rev()
        .filter_map(|i| nodes[i].take())
        .map(|n| (n.id, n.message))
        .collect();
    Loaded { branch }
}

/// The messages of the current branch.
pub fn messages(text: &str) -> Vec<Message> {
    read(text).branch.into_iter().map(|(_, m)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Part;

    const V1: &str = include_str!("../../tests/fixtures/session/v1.jsonl");
    /// Frozen like v1.jsonl: two branches, a rewind, a repeated id and a line from the future.
    const BRANCHES: &str = include_str!("../../tests/fixtures/session/branches.jsonl");

    fn texts(text: &str) -> Vec<String> {
        messages(text)
            .iter()
            .map(|m| match m {
                Message::User { parts } => Part::text_of(parts),
                Message::Assistant(a) => Part::text_of(&a.parts),
                Message::Tool(t) => t.content.clone(),
            })
            .collect()
    }

    #[test]
    fn a_line_it_cannot_read_is_skipped() {
        let text = format!("{V1}{{\"type\":\"from_the_future\"}}\nnot json\n");
        assert_eq!(messages(&text).len(), messages(V1).len());
        assert_eq!(messages(V1).len(), 10);
    }

    #[test]
    fn the_branch_follows_parents_and_rewinds_and_a_repeated_id_means_the_latest() {
        assert_eq!(
            texts(BRANCHES),
            [
                "first",
                "first reply",
                "second, again",
                "a repeated id",
                "third"
            ]
        );
        let ids: Vec<String> = read(BRANCHES)
            .branch
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["a1", "a2", "a5", "a1", "a6"]);
    }

    #[test]
    fn a_rewind_to_the_start_leaves_nothing_until_the_next_message() {
        let rewind = r#"{"type":"rewind","ts":"2026-10-16T10:00:09Z","to":null,"from":"a6"}"#;
        assert!(messages(&format!("{BRANCHES}{rewind}\n")).is_empty());
        let again = r#"{"type":"message","id":"b1","parent":null,"ts":"2026-10-16T10:00:10Z","message":{"role":"user","parts":[{"type":"text","text":"fresh"}]}}"#;
        assert_eq!(texts(&format!("{BRANCHES}{rewind}\n{again}\n")), ["fresh"]);
    }
}
