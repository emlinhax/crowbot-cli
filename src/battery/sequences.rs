//! Tool calls that come in the same order again and again: candidates for one call.

use super::Counts;
use super::load::Run;

/// Every run of `n` consecutive calls within a run, by tool names.
pub fn ngrams(runs: &[Run], n: usize) -> Counts {
    let mut out = Counts::default();
    for run in runs {
        let names: Vec<&str> = run.calls.iter().map(|c| c.name.as_str()).collect();
        for window in names.windows(n) {
            out.add(window.join(" → "), &run.record.task);
        }
    }
    out
}
