//! Per tool: how often it was called, failed or was refused, and how often a call came again
//! unchanged.

use std::collections::{BTreeMap, HashSet};

use super::Counts;
use super::load::Run;

pub struct ToolRow {
    pub name: String,
    pub calls: usize,
    /// Ran and failed.
    pub errors: usize,
    /// Never ran; `ToolResult::refused` says why.
    pub refused: usize,
}

#[derive(Default)]
pub struct Tools {
    /// Most called first.
    pub rows: Vec<ToolRow>,
    /// By `<why> · <tool>`.
    pub refusals: Counts,
    /// The same call again after it failed or was refused, by tool.
    pub retried: Counts,
    /// The same call again after it succeeded, by tool.
    pub repeated: Counts,
}

pub fn tally(runs: &[Run]) -> Tools {
    let mut out = Tools::default();
    let mut rows: BTreeMap<&str, ToolRow> = BTreeMap::new();
    for run in runs {
        let mut failed: HashSet<(&str, &str)> = HashSet::new();
        let mut succeeded: HashSet<(&str, &str)> = HashSet::new();
        for call in &run.calls {
            let row = rows.entry(&call.name).or_insert_with(|| ToolRow {
                name: call.name.clone(),
                calls: 0,
                errors: 0,
                refused: 0,
            });
            row.calls += 1;
            let key = (call.name.as_str(), call.arguments.as_str());
            if failed.contains(&key) {
                out.retried.add(&call.name, &call.arguments);
            } else if succeeded.contains(&key) {
                out.repeated.add(&call.name, &call.arguments);
            }
            let Some(result) = &call.result else {
                continue;
            };
            if let Some(why) = &result.refused {
                row.refused += 1;
                out.refusals
                    .add(format!("{why} · {}", call.name), &call.arguments);
            } else if result.is_error {
                row.errors += 1;
            }
            if result.is_error {
                failed.insert(key);
            } else {
                succeeded.insert(key);
            }
        }
    }
    out.rows = rows.into_values().collect();
    out.rows
        .sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;

    #[test]
    fn failures_refusals_and_repeats_are_told_apart() {
        let tools = tally(&fixture::battery().runs);
        let bash = &tools.rows[0];
        assert_eq!(
            (bash.name.as_str(), bash.calls, bash.errors, bash.refused),
            ("bash", 4, 1, 1)
        );
        assert_eq!(tools.refusals.get("unknown_tool · nope"), 1);
        assert_eq!(tools.refusals.get("denied · bash"), 1);
        assert_eq!((tools.retried.get("bash"), tools.repeated.total()), (1, 0));
    }
}
