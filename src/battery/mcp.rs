//! What the battery's MCP servers made of the model's calls, from each simulated server's own log,
//! and how the model spread its calls over replies.

use std::collections::BTreeMap;

use super::Counts;
use super::load::{Call, Run};

pub struct McpRow {
    /// As crowbot named it to the model: `<server>__<tool>`.
    pub name: String,
    pub calls: usize,
    /// By the server's word for how a call went.
    pub outcomes: BTreeMap<String, usize>,
}

#[derive(Default)]
pub struct Mcp {
    /// Most called first.
    pub rows: Vec<McpRow>,
    /// Arguments a server refused, by tool and the error's shape (its values blanked).
    pub mistakes: Counts,
    /// Calls made before the server was ready, by tool.
    pub unready: Counts,
    /// Replies that made one call to the same tool as the reply before, which could have
    /// joined it, by tool.
    pub one_per_reply: Counts,
    /// Calls to one tool made together in one reply, by tool.
    pub together: Counts,
}

pub fn tally(runs: &[Run]) -> Mcp {
    let mut out = Mcp::default();
    let mut rows: BTreeMap<String, McpRow> = BTreeMap::new();
    for run in runs {
        for served in &run.served {
            let name = served.name();
            let row = rows.entry(name.clone()).or_insert_with(|| McpRow {
                name: name.clone(),
                calls: 0,
                outcomes: BTreeMap::new(),
            });
            row.calls += 1;
            *row.outcomes.entry(served.outcome.clone()).or_default() += 1;
            let example = served.args.to_string();
            match served.outcome.as_str() {
                "invalid_args" => {
                    let error = served.error.as_deref().unwrap_or_default();
                    out.mistakes
                        .add(format!("{name}: {}", shape(error)), &example);
                }
                "precondition" => out.unready.add(&name, &example),
                _ => {}
            }
        }
        spread(run, &mut out);
    }
    out.rows = rows.into_values().collect();
    out.rows
        .sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
    out
}

/// Counts, for MCP tools, replies whose one call could have joined the reply before (its one
/// call, to the same tool, worked, so the next did not depend on what went wrong), and calls to
/// one tool made together.
fn spread(run: &Run, out: &mut Mcp) {
    let mut by_reply: BTreeMap<usize, Vec<&Call>> = BTreeMap::new();
    for call in &run.calls {
        by_reply.entry(call.reply).or_default().push(call);
    }
    let mut last_single: Option<(usize, &Call)> = None;
    for (&reply, calls) in &by_reply {
        let mcp: Vec<&Call> = calls
            .iter()
            .copied()
            .filter(|c| c.name.contains("__"))
            .collect();
        if calls.len() == 1 && mcp.len() == 1 {
            let call = mcp[0];
            if let Some((before, last)) = last_single
                && before + 1 == reply
                && last.name == call.name
                && last.args != call.args
                && last.result.as_ref().is_some_and(|r| !r.is_error)
            {
                out.one_per_reply.add(&call.name, &call.arguments);
            }
            last_single = Some((reply, call));
            continue;
        }
        last_single = None;
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for call in mcp {
            *seen.entry(call.name.as_str()).or_default() += 1;
        }
        for (name, n) in seen.into_iter().filter(|(_, n)| *n > 1) {
            for _ in 0..n {
                out.together.add(name, "");
            }
        }
    }
}

/// An error with its values blanked, so the same mistake with other values counts once: quoted
/// text becomes `"…"`, and digits after `got`, `of` or `:` a `N`.
fn shape(error: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut digits = false;
    for c in error.chars() {
        match c {
            '"' => {
                if !quoted {
                    out.push_str("\"…\"");
                }
                quoted = !quoted;
            }
            _ if quoted => {}
            c if c.is_ascii_digit() || (digits && c.is_ascii_hexdigit()) => {
                if !digits {
                    out.push('N');
                    digits = true;
                }
            }
            c => {
                digits = false;
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;

    #[test]
    fn the_servers_outcomes_mistakes_and_the_spread_of_calls_are_counted() {
        let mcp = tally(&fixture::battery().runs);
        let decompile = mcp
            .rows
            .iter()
            .find(|r| r.name == "re__decompile_function")
            .unwrap();
        assert_eq!(decompile.calls, 4);
        assert_eq!(decompile.outcomes["ok"], 2);
        assert_eq!(
            mcp.mistakes
                .get("re__decompile_function: arguments.address: expected string, got integer N"),
            1
        );
        assert_eq!(mcp.unready.get("re__decompile_function"), 1);
        assert_eq!(mcp.one_per_reply.get("re__decompile_function"), 1);
        assert_eq!(mcp.together.get("re__set_comment"), 2);
    }

    #[test]
    fn an_errors_values_are_blanked() {
        assert_eq!(
            shape("arguments.state: \"OPEN\" is not one of \"open\", \"closed\""),
            "arguments.state: \"…\" is not one of \"…\", \"…\""
        );
        assert_eq!(shape("got integer 4198400"), "got integer N");
        assert_eq!(
            shape("No function starts at 0x401895."),
            "No function starts at NxN."
        );
    }
}
