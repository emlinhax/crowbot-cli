//! One line of a session file. Fields are only added, with defaults, so older files load;
//! tests/fixtures/session/v1.jsonl is frozen to prove it.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::conversation::message::{Message, Usage};

/// Bump only for a change old files can't load under; additions don't bump it.
pub const FORMAT_VERSION: u32 = 1;

// Built once per line, written or read; boxing the big variant would only add noise.
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
        /// On a prompt: what it was sent under, so a resumed session carries on the same way.
        /// Kept beside the message, which is what the model sees.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<State>,
    },
    /// The project's files before prompt `of` first changed them, and once its run ended, as
    /// shadow git trees (src/snapshot/). The last line for a prompt wins.
    Snapshot {
        ts: Timestamp,
        of: String,
        #[serde(flatten)]
        trees: Snap,
    },
    /// Everything before message `kept`, on the branch that ends at `head`, summarised for the
    /// model; it is sent the summary and what follows, word for word. A rewind to before `head`
    /// leaves it behind.
    Compaction {
        ts: Timestamp,
        head: String,
        kept: String,
        #[serde(flatten)]
        summary: Summary,
    },
    /// Moves the head back to `to` (`None`: before the first message); what came after stays in
    /// the file as a branch.
    Rewind {
        ts: Timestamp,
        to: Option<String>,
        /// The head it moved from.
        from: Option<String>,
    },
}

/// A summary of earlier turns and how it came about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub summary: String,
    /// Made on its own before a request, rather than asked for with /compact.
    #[serde(default)]
    pub auto: bool,
    /// Made because crowbot refused a request as too long.
    #[serde(default)]
    pub overflow: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// A prompt's two snapshots: before its first change, and when its run ended (`None` until then).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snap {
    pub before: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// The settings a prompt went out with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    /// `None` where it was never recorded, as for a state read off an old file's last reply.
    #[serde(default)]
    pub mode: Option<String>,
}

/// Random ids, readable in a file. A reader resolves a repeat to the nearest earlier entry, so
/// one is harmless; with 64 bits it is also all but impossible.
pub fn new_id() -> String {
    format!("{:016x}", fastrand::u64(..))
}

/// Every line of a session file's `text`, naming the line that does not parse.
#[cfg(test)]
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .enumerate()
        .map(|(i, l)| serde_json::from_str(l).unwrap_or_else(|e| panic!("line {}: {e}", i + 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::{Finish, Part};

    /// Written by v1 builds and never edited: a line that stops parsing is a breaking change.
    const V1: &str = include_str!("../../tests/fixtures/session/v1.jsonl");

    #[test]
    fn every_v1_line_still_loads() {
        let entries = parse(V1);
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
