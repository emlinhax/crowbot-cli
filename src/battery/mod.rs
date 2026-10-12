//! Reading a tool-use battery's records back (NEXT_PLAN.md M9): what the model called, how, and
//! what the gate would have said, counted so the report can rank what to make easier.

pub mod load;
pub mod record;
pub mod replay;
pub mod report;
pub mod sequences;
pub mod shell;
pub mod tools;

use std::collections::BTreeMap;

/// How often each thing happened, with the first example of it.
#[derive(Default)]
pub struct Counts(BTreeMap<String, Count>);

pub struct Count {
    pub n: usize,
    pub example: String,
}

impl Counts {
    pub fn add(&mut self, key: impl Into<String>, example: &str) {
        self.0
            .entry(key.into())
            .or_insert_with(|| Count {
                n: 0,
                example: example.to_owned(),
            })
            .n += 1;
    }

    pub fn total(&self) -> usize {
        self.0.values().map(|c| c.n).sum()
    }

    pub fn get(&self, key: &str) -> usize {
        self.0.get(key).map_or(0, |c| c.n)
    }

    /// The `limit` most frequent, ties in key order.
    pub fn top(&self, limit: usize) -> Vec<(&str, &Count)> {
        let mut all: Vec<(&str, &Count)> = self.0.iter().map(|(k, c)| (k.as_str(), c)).collect();
        all.sort_by(|a, b| b.1.n.cmp(&a.1.n).then(a.0.cmp(b.0)));
        all.truncate(limit);
        all
    }
}

#[cfg(test)]
pub mod fixture {
    //! tests/fixtures/battery/: an AUTO run that reads, retries a failed test and edits, a PLAN
    //! run refused a command PLAN does not allow and a tool that does not exist, a run whose only
    //! reply was a 502, and a run that never started.

    use super::load::{self, Battery};
    use super::report::Words;

    pub fn battery() -> Battery {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/battery");
        load::load(&dir).unwrap()
    }

    pub fn words() -> Words {
        toml::from_str(include_str!("../../data/commands/battery.toml")).unwrap()
    }
}
