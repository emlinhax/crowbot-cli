//! What a run leaves behind: its files, each scrubbed of the key and checked for it, and one
//! line in results.jsonl.

use std::io::Write as _;
use std::path::Path;

use anyhow::ensure;
use serde::Serialize;

/// One line of results.jsonl: everything the analyzer needs that the session file lacks.
#[derive(Default, Serialize)]
pub struct Record {
    pub task: String,
    pub repo: String,
    pub mode: String,
    pub model: String,
    pub rep: u32,
    pub exit: Option<i32>,
    /// What ended the run early: `turns`, `run_cap`, `wall` or `cap` (see bench/battery.toml).
    pub stopped_by: Option<&'static str>,
    pub check: bool,
    pub turns: u32,
    pub cost_micros: u64,
    pub wall_ms: u64,
    /// The plan file as crowbot spelled it, for replaying PLAN's gate.
    pub plan_file: String,
    /// The run's files, relative to the battery's directory.
    pub dir: String,
    /// Why the run could not happen at all, e.g. a repo that is not set up.
    pub error: Option<String>,
}

pub fn save(dir: &Path, name: &str, text: &str, key: &str) -> anyhow::Result<()> {
    ensure!(!key.is_empty(), "no key to scrub");
    let path = dir.join(name);
    std::fs::write(&path, text.replace(key, "<key>"))?;
    ensure!(
        !std::fs::read_to_string(&path)?.contains(key),
        "{name} still holds the key"
    );
    Ok(())
}

pub fn append(results: &Path, record: &Record) -> anyhow::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(results)?;
    writeln!(file, "{}", serde_json::to_string(record)?)?;
    Ok(())
}
