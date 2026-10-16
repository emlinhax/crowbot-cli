//! The recorded asks put through the gate again: what a mode would have asked about or refused
//! had the runs been made in it, and what was refused live in each mode the runs used.

use std::collections::BTreeMap;

use super::Counts;
use super::load::Run;
use super::shell;
use crate::mode::Mode;
use crate::permission::gate::{Ask, Decision};
use crate::permission::rule::Rule;

pub struct Replayed {
    pub mode: String,
    pub asks: Counts,
    pub denies: Counts,
}

/// Each of `modes` replayed over every run.
pub fn replay(
    runs: &[Run],
    rules: &[Rule],
    modes: &[&Mode],
    subcommands: &[String],
) -> Vec<Replayed> {
    modes
        .iter()
        .map(|mode| {
            let mut out = Replayed {
                mode: mode.id.clone(),
                asks: Counts::default(),
                denies: Counts::default(),
            };
            for run in runs {
                let locks = mode.locks(&run.record.plan_file);
                let policy = mode.policy(rules, &locks);
                for result in run.calls.iter().filter_map(|c| c.result.as_ref()) {
                    for (ask, pattern) in patterns(&result.asks) {
                        let one = [Ask::new(&ask.permission, pattern)];
                        let counts = match policy.decide(&one) {
                            Decision::Allow => continue,
                            Decision::Ask => &mut out.asks,
                            Decision::Deny(_) => &mut out.denies,
                        };
                        counts.add(label(&ask.permission, pattern, subcommands), pattern);
                    }
                }
            }
            out
        })
        .collect()
}

/// What was refused live because the gate said no or nobody could answer, by the mode the run
/// was in; only the patterns that mode would not allow are counted.
pub fn refused_live(
    runs: &[Run],
    rules: &[Rule],
    subcommands: &[String],
) -> BTreeMap<String, Counts> {
    let mut out: BTreeMap<String, Counts> = BTreeMap::new();
    for run in runs {
        let Some(mode) = crate::mode::get(&run.record.mode) else {
            continue;
        };
        let locks = mode.locks(&run.record.plan_file);
        let policy = mode.policy(rules, &locks);
        for result in run.calls.iter().filter_map(|c| c.result.as_ref()) {
            if !matches!(
                result.refused.as_deref(),
                Some("denied" | "non_interactive")
            ) {
                continue;
            }
            let counts = out.entry(run.record.mode.clone()).or_default();
            for (ask, pattern) in patterns(&result.asks) {
                if policy.decide(&[Ask::new(&ask.permission, pattern)]) != Decision::Allow {
                    counts.add(label(&ask.permission, pattern, subcommands), pattern);
                }
            }
        }
    }
    out
}

fn patterns(asks: &[Ask]) -> impl Iterator<Item = (&Ask, &str)> {
    asks.iter()
        .flat_map(|a| a.patterns.iter().map(move |p| (a, p.as_str())))
}

/// What an allowlist rule would name: a command by how it starts, anything else by permission.
fn label(permission: &str, pattern: &str, subcommands: &[String]) -> String {
    if permission.starts_with("bash") {
        format!("{permission} `{}`", shell::lead(pattern, subcommands))
    } else {
        permission.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;
    use crate::config::settings;

    #[test]
    fn manual_would_ask_what_auto_ran_and_plan_refused_live() {
        let words = fixture::words();
        let runs = fixture::battery().runs;
        let rules = settings::default_rules();
        let manual = crate::mode::get("manual").unwrap();
        let replayed = replay(&runs, &rules, &[manual], &words.subcommands);
        let asks = &replayed[0].asks;
        assert_eq!(asks.get("bash `cargo test`"), 2);
        assert_eq!(asks.get("bash `sed`"), 1);
        assert_eq!(asks.get("bash `python3`"), 1);
        assert_eq!(asks.get("edit"), 1);
        assert_eq!(asks.get("search"), 0, "reading is allowed");
        let live = refused_live(&runs, &rules, &words.subcommands);
        assert_eq!(live["plan"].get("bash `python3`"), 1);
        assert!(!live.contains_key("auto"));
    }
}
