//! The conversation as kept: in memory (transcript.rs), as a JSONL file (store.rs, entry.rs),
//! read back (reader.rs), and found again (list.rs, shown by table.rs).

pub mod entry;
pub mod list;
pub mod reader;
pub mod store;
pub mod table;
pub mod transcript;
pub mod undo;
