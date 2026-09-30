//! One line of a session file. Entries are only added and optional fields default, so older
//! files keep loading (tests/fixtures/session/v1.jsonl is frozen to prove it).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::message::Message;

/// Bump only for a change old files can't load under; additions don't bump it.
pub const FORMAT_VERSION: u32 = 1;

// CEILING: an older build fails on a new type, role, part or finish. Upgrade: the reader (M5)
// skips lines it can't parse.
// Written once and dropped; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    /// First line of every file.
    Session {
        v: u32,
        id: String,
        created: Timestamp,
        cwd: String,
        #[serde(default)]
        crowbot: String,
    },
    Message {
        id: String,
        /// The entry this one follows; lets a file hold branches (undo, retry) without deleting.
        #[serde(default)]
        parent: Option<String>,
        ts: Timestamp,
        message: Message,
    },
}

/// Short random ids, readable in a file.
/// CEILING: 32 random bits, ~1% chance of a repeat in a 9k-entry file. Upgrade: a per-file
/// counter or u64 once a reader follows parents.
pub fn new_id() -> String {
    format!("{:08x}", fastrand::u32(..))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Finish, Part};

    /// Written by v1 builds and never edited: a line that stops parsing is a breaking change.
    const V1: &str = include_str!("../../tests/fixtures/session/v1.jsonl");

    #[test]
    fn every_v1_line_still_loads() {
        let entries: Vec<Entry> = V1
            .lines()
            .enumerate()
            .map(|(i, l)| serde_json::from_str(l).unwrap_or_else(|e| panic!("line {}: {e}", i + 1)))
            .collect();
        assert!(matches!(&entries[0], Entry::Session { v: 1, crowbot, .. } if crowbot == "0.1.0"));
        let Entry::Message {
            message: Message::Assistant(a),
            parent,
            ..
        } = &entries[2]
        else {
            panic!("line 3 is an assistant message");
        };
        assert_eq!(parent.as_deref(), Some("00000001"));
        assert!(matches!(a.parts[0], Part::Reasoning { .. }));
        assert_eq!(
            a.tool_calls().next().unwrap().arguments,
            r#"{"path":"a.rs"}"#
        );
        assert_eq!(a.usage.as_ref().unwrap().cost_micros, 4840);
        let Entry::Message {
            message: Message::Assistant(failed),
            ..
        } = &entries[4]
        else {
            panic!("line 5 is an assistant message");
        };
        let error = failed.error.as_ref().unwrap();
        assert_eq!(
            (error.kind.as_str(), error.status),
            ("upstream_error", Some(502))
        );
        let finishes: Vec<Finish> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Message {
                    message: Message::Assistant(a),
                    ..
                } => Some(a.finish),
                _ => None,
            })
            .collect();
        for finish in [
            Finish::Done,
            Finish::ToolCalls,
            Finish::Length,
            Finish::Error,
            Finish::Aborted,
        ] {
            assert!(finishes.contains(&finish), "{finish:?}");
        }
    }
}
