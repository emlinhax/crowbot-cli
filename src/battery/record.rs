//! One line of a battery's results.jsonl. The runner (tests/battery/) includes this very file to
//! write it, so writer and reader share one shape; it may use serde and nothing of the crate.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    pub task: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub model: String,
    pub rep: u32,
    #[serde(default)]
    pub exit: Option<i32>,
    /// What ended the run early: `turns`, `run_cap`, `wall` or `cap` (bench/battery.toml).
    #[serde(default)]
    pub stopped_by: Option<String>,
    /// The task's check passed.
    #[serde(default)]
    pub check: bool,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub cost_micros: u64,
    #[serde(default)]
    pub wall_ms: u64,
    /// The plan file as crowbot spelled it, for replaying PLAN's gate.
    #[serde(default)]
    pub plan_file: String,
    /// The run's files, relative to the battery's directory.
    #[serde(default)]
    pub dir: String,
    /// Why the run could not happen at all, e.g. a repo that is not set up.
    #[serde(default)]
    pub error: Option<String>,
}
