//! One line of a session file. Entries only ever get added, and every optional field has a
//! default, so files written by older and newer builds keep loading.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::message::Message;

pub const FORMAT_VERSION: u32 = 1;

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

/// Short random ids: unique within a session, readable in a file.
pub fn new_id() -> String {
    format!("{:08x}", fastrand::u32(..))
}
