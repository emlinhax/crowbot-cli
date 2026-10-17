//! Steps that come in the same order again and again: candidates for one call. A step is one
//! reply's calls, so calls the model already made together count once, not as a sequence.

use std::collections::BTreeMap;

use super::Counts;
use super::load::Run;

/// Every run of `n` consecutive replies that made calls, each by its tools' names (each name
/// once, joined with `+` when a reply called several).
pub fn ngrams(runs: &[Run], n: usize) -> Counts {
    let mut out = Counts::default();
    for run in runs {
        let mut replies: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
        for call in &run.calls {
            let names = replies.entry(call.reply).or_default();
            if !names.contains(&call.name.as_str()) {
                names.push(&call.name);
            }
        }
        let steps: Vec<String> = replies
            .into_values()
            .map(|names| names.join(" + "))
            .collect();
        for window in steps.windows(n) {
            out.add(window.join(" → "), &run.record.task);
        }
    }
    out
}
