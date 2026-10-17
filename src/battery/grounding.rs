//! Whether a run grounded its answer by running or scripting a check — a test, a program, a short
//! script, a debugger — rather than working it out in its head, read from the bash calls it made.
//! The empirical commands are data (the `[[ground]]` list in data/commands/battery.toml), named the
//! way the smells are.

use serde::Deserialize;

use super::Counts;
use super::load::{Call, Run};
use super::shell;
use crate::permission::bash::split;

/// A command that grounds an answer empirically: it starts with `words` and contains `contains`, of
/// those it gives. Unlike a smell it carries no label — grounding is one signal per run, not a
/// per-command count.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ground {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub contains: Option<String>,
}

#[derive(Default)]
pub struct Grounding {
    /// Each started run as grounded or not, its task the example.
    pub split: Counts,
    /// Runs that missed their check having run or scripted nothing, by task.
    pub misses: Counts,
}

pub fn tally(runs: &[Run], ground: &[Ground]) -> Grounding {
    let mut out = Grounding::default();
    for run in runs.iter().filter(|r| r.record.error.is_none()) {
        let empirical = run.calls.iter().any(|c| grounds(c, ground));
        let key = match empirical {
            true => "grounded by running or scripting",
            false => "answered without running or scripting",
        };
        out.split.add(key, &run.record.task);
        if !empirical && !run.record.check {
            out.misses.add(run.record.task.clone(), &run.record.task);
        }
    }
    out
}

/// Whether a bash call runs one of the empirical commands.
fn grounds(call: &Call, ground: &[Ground]) -> bool {
    if call.name != "bash" {
        return false;
    }
    let Some(line) = call.args["command"].as_str() else {
        return false;
    };
    split::split(line).commands.iter().any(|command| {
        let words = shell::words(command);
        ground
            .iter()
            .any(|g| shell::command_matches(&g.words, g.contains.as_deref(), &words, command))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;

    #[test]
    fn a_run_that_ran_or_scripted_a_check_is_grounded_the_rest_are_not() {
        let words = fixture::words();
        let grounding = tally(&fixture::battery().runs, &words.ground);
        // fix-it ran `cargo test`, plan-it ran `python3`; flaky and mcp-it ran nothing. The run
        // that never started is left out.
        assert_eq!(grounding.split.get("grounded by running or scripting"), 2);
        assert_eq!(
            grounding.split.get("answered without running or scripting"),
            2
        );
        // Only flaky missed its check having run or scripted nothing; plan-it ran but still missed.
        assert_eq!(grounding.misses.get("flaky"), 1);
        assert_eq!(grounding.misses.total(), 1);
    }
}
