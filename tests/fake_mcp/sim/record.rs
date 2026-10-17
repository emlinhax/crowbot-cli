//! What a simulated server keeps for the battery to read: its data and flags after every change
//! (`<name>.json`), and every call with how it went (`<name>.calls.jsonl`).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::act::{Collections, Outcome};

pub struct Record {
    state: PathBuf,
    calls: File,
}

impl Record {
    pub fn open(dir: &Path, name: &str) -> Self {
        std::fs::create_dir_all(dir).expect("the record folder can be made");
        let calls = File::create(dir.join(format!("{name}.calls.jsonl")))
            .expect("the call log can be written");
        Self {
            state: dir.join(format!("{name}.json")),
            calls,
        }
    }

    pub fn call(&mut self, tool: &str, args: &Value, outcome: Outcome, error: Option<&str>) {
        let line = json!({"tool": tool, "args": args, "outcome": outcome, "error": error});
        let _ = writeln!(self.calls, "{line}");
    }

    pub fn state(&self, flags: &BTreeSet<String>, data: &Collections) {
        let state = json!({"flags": flags, "collections": data});
        let text = serde_json::to_string_pretty(&state).expect("state is JSON");
        std::fs::write(&self.state, text).expect("the state can be written");
    }
}
