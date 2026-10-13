//! What a run leaves behind: its files, each scrubbed of the key and checked for it, and its
//! line in results.jsonl (whose shape src/battery/record.rs defines for writer and reader).

use std::io::Write as _;
use std::path::Path;

use anyhow::ensure;

pub use crate::shape::Record;

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
