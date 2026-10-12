//! The report: every count as a markdown table, then the candidate changes ranked by the count
//! behind each, and how the numbers moved against an earlier battery.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::load::{Battery, Run};
use super::replay::{self, Replayed};
use super::sequences;
use super::shell::{self, Shell, Smell};
use super::tools::{self, Tools};
use super::{Count, Counts};
use crate::api::error::ErrorInfo;
use crate::mode;
use crate::permission::rule::Rule;
use crate::text::shorten;
use crate::text::template::fill;

/// How much of an example a table cell shows.
const EXAMPLE_CHARS: usize = 60;

/// The report's part of data/commands/battery.toml.
#[derive(Deserialize)]
pub struct Words {
    pub top: usize,
    pub candidates: usize,
    pub subcommands: Vec<String>,
    pub replay: Vec<String>,
    pub never_propose: Vec<String>,
    pub smell: Vec<Smell>,
    pub text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    title: String,
    summary: String,
    missing: String,
    tasks: String,
    tasks_columns: Vec<String>,
    failed: String,
    tools: String,
    tools_columns: Vec<String>,
    refusals: String,
    repeats: String,
    repeats_columns: Vec<String>,
    shell: String,
    first: String,
    first_two: String,
    flags: String,
    filters: String,
    smells: String,
    smells_columns: Vec<String>,
    sequences: String,
    pairs: String,
    triples: String,
    replay: String,
    replay_asks: String,
    replay_denies: String,
    live: String,
    count_columns: Vec<String>,
    candidates: String,
    candidate_smell: String,
    candidate_ask: String,
    candidate_live: String,
    candidate_refusal: String,
    candidate_retry: String,
    candidate_sequence: String,
    against: String,
    against_columns: Vec<String>,
    against_rows: Vec<String>,
    pub saved: String,
}

/// Refusals that say the model misread a tool, not that the gate said no.
const MISREAD: &[&str] = &["invalid_arguments", "bad_json", "unknown_tool", "check"];

struct Stats {
    tools: Tools,
    shell: Shell,
    pairs: Counts,
    triples: Counts,
    replayed: Vec<Replayed>,
    live: BTreeMap<String, Counts>,
}

impl Stats {
    fn of(battery: &Battery, words: &Words, rules: &[Rule]) -> Self {
        let runs = &battery.runs;
        let modes: Vec<&mode::Mode> = words.replay.iter().filter_map(|m| mode::get(m)).collect();
        Self {
            tools: tools::tally(runs),
            shell: shell::tally(runs, &words.smell, &words.subcommands),
            pairs: sequences::ngrams(runs, 2),
            triples: sequences::ngrams(runs, 3),
            replayed: replay::replay(runs, rules, &modes, &words.subcommands),
            live: replay::refused_live(runs, rules, &words.subcommands),
        }
    }
}

pub fn render(this: &Battery, that: Option<&Battery>, words: &Words, rules: &[Rule]) -> String {
    let t = &words.text;
    let stats = Stats::of(this, words, rules);
    let runs = &this.runs;
    let mut out: Vec<String> = Vec::new();

    let started: Vec<&Run> = runs.iter().filter(|r| r.record.error.is_none()).collect();
    let tasks: BTreeMap<&str, Vec<&Run>> = started.iter().fold(BTreeMap::new(), |mut m, r| {
        m.entry(r.record.task.as_str())
            .or_insert_with(Vec::new)
            .push(*r);
        m
    });
    let tokens: u64 = started.iter().map(|r| r.usage.input + r.usage.output).sum();
    let cost: u64 = started.iter().map(|r| r.record.cost_micros).sum();
    out.push(fill(&t.title, &[("name", &this.name)]));
    out.push(fill(
        &t.summary,
        &[
            ("model", &this.meta.model),
            ("runs", &started.len().to_string()),
            ("tasks", &tasks.len().to_string()),
            ("done", &done(&started)),
            (
                "failed",
                &started
                    .iter()
                    .filter(|r| r.failed.is_some())
                    .count()
                    .to_string(),
            ),
            ("cost", &dollars(cost)),
            ("tokens", &tokens.to_string()),
            ("crowbot", short_rev(&this.meta.crowbot)),
        ],
    ));
    let missing: Vec<String> = runs
        .iter()
        .filter_map(|r| {
            r.record.error.as_ref().map(|e| {
                format!(
                    "{} #{} ({})",
                    r.record.task,
                    r.record.rep,
                    shorten::line(e, EXAMPLE_CHARS)
                )
            })
        })
        .collect();
    if !missing.is_empty() {
        out.push(fill(&t.missing, &[("list", &missing.join(", "))]));
    }

    out.push(t.tasks.clone());
    out.push(table(
        &t.tasks_columns,
        tasks
            .iter()
            .map(|(task, runs)| {
                let n = runs.len() as f64;
                let mut stops: Counts = Counts::default();
                for r in runs {
                    if let Some(s) = &r.record.stopped_by {
                        stops.add(s.as_str(), "");
                    }
                    if let Some(e) = &r.failed {
                        stops.add(failure(e), "");
                    }
                }
                vec![
                    (*task).to_owned(),
                    runs[0].record.mode.clone(),
                    done(runs),
                    one_place(runs.iter().map(|r| f64::from(r.record.turns)).sum::<f64>() / n),
                    one_place(runs.iter().map(|r| r.calls.len() as f64).sum::<f64>() / n),
                    dollars(runs.iter().map(|r| r.record.cost_micros).sum()),
                    format!(
                        "{}s",
                        runs.iter().map(|r| r.record.wall_ms).sum::<u64>()
                            / 1000
                            / runs.len() as u64
                    ),
                    stops
                        .top(usize::MAX)
                        .iter()
                        .map(|(k, c)| format!("{k}×{}", c.n))
                        .collect::<Vec<_>>()
                        .join(" "),
                ]
            })
            .collect(),
    ));

    let mut failures = Counts::default();
    for r in &started {
        if let Some(e) = &r.failed {
            let what = format!("{}: {}", failure(e), e.message.trim());
            failures.add(what, &format!("{} #{}", r.record.task, r.record.rep));
        }
    }
    if failures.total() > 0 {
        out.push(t.failed.clone());
        out.push(counts(&t.count_columns, &failures, words.top));
    }

    let per_run = |n: usize| one_place(n as f64 / started.len().max(1) as f64);
    out.push(t.tools.clone());
    out.push(table(
        &t.tools_columns,
        stats
            .tools
            .rows
            .iter()
            .map(|row| {
                vec![
                    row.name.clone(),
                    row.calls.to_string(),
                    per_run(row.calls),
                    row.errors.to_string(),
                    row.refused.to_string(),
                ]
            })
            .collect(),
    ));
    out.push(t.refusals.clone());
    out.push(counts(&t.count_columns, &stats.tools.refusals, words.top));
    out.push(t.repeats.clone());
    let mut repeated: Vec<&str> = stats
        .tools
        .rows
        .iter()
        .map(|r| r.name.as_str())
        .filter(|n| stats.tools.retried.get(n) + stats.tools.repeated.get(n) > 0)
        .collect();
    repeated.sort_by_key(|n| std::cmp::Reverse(stats.tools.retried.get(n)));
    out.push(table(
        &t.repeats_columns,
        repeated
            .iter()
            .map(|n| {
                vec![
                    (*n).to_owned(),
                    stats.tools.retried.get(n).to_string(),
                    stats.tools.repeated.get(n).to_string(),
                ]
            })
            .collect(),
    ));

    let sh = &stats.shell;
    out.push(fill(
        &t.shell,
        &[
            ("calls", &sh.calls.to_string()),
            ("complex", &sh.complex.to_string()),
        ],
    ));
    for (heading, which) in [
        (&t.first, &sh.first),
        (&t.first_two, &sh.first_two),
        (&t.flags, &sh.flags),
        (&t.filters, &sh.filters),
    ] {
        out.push(heading.clone());
        out.push(counts(&t.count_columns, which, words.top));
    }
    out.push(t.smells.clone());
    out.push(table(
        &t.smells_columns,
        sh.smells
            .top(words.top)
            .iter()
            .map(|(label, c)| {
                vec![
                    format!("`{label}`"),
                    instead(&words.smell, label).to_owned(),
                    c.n.to_string(),
                    example(c),
                ]
            })
            .collect(),
    ));

    out.push(t.sequences.clone());
    out.push(t.pairs.clone());
    out.push(counts(&t.count_columns, &stats.pairs, words.top));
    out.push(t.triples.clone());
    out.push(counts(&t.count_columns, &stats.triples, words.top));

    for replayed in &stats.replayed {
        out.push(fill(&t.replay, &[("mode", mode_label(&replayed.mode))]));
        out.push(t.replay_asks.clone());
        out.push(counts(&t.count_columns, &replayed.asks, words.top));
        out.push(t.replay_denies.clone());
        out.push(counts(&t.count_columns, &replayed.denies, words.top));
    }
    for (mode, refused) in stats.live.iter().filter(|(_, c)| c.total() > 0) {
        out.push(fill(&t.live, &[("mode", mode_label(mode))]));
        out.push(counts(&t.count_columns, refused, words.top));
    }

    out.push(t.candidates.clone());
    out.push(
        candidates(&stats, words)
            .iter()
            .take(words.candidates)
            .enumerate()
            .map(|(i, (_, line))| format!("{}. {line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    if let Some(that) = that {
        out.push(fill(&t.against, &[("name", &that.name)]));
        let theirs = Stats::of(that, words, rules);
        let (a, b) = (measures(this, &stats), measures(that, &theirs));
        out.push(table(
            &t.against_columns,
            t.against_rows
                .iter()
                .zip(a.iter().zip(&b))
                .map(|(label, (x, y))| {
                    vec![
                        label.clone(),
                        one_place(*x),
                        one_place(*y),
                        format!("{:+.1}", x - y),
                    ]
                })
                .collect(),
        ));
    }
    out.join("\n\n") + "\n"
}

/// Ranked by the count behind each, most first.
fn candidates(stats: &Stats, words: &Words) -> Vec<(usize, String)> {
    let t = &words.text;
    let mut all: Vec<(usize, String)> = Vec::new();
    let mut push = |template: &str, n: usize, extra: &[(&str, &str)]| {
        let count = n.to_string();
        let mut values = vec![("n", count.as_str())];
        values.extend_from_slice(extra);
        all.push((n, fill(template, &values)));
    };
    for (label, c) in stats.shell.smells.top(usize::MAX) {
        push(
            &t.candidate_smell,
            c.n,
            &[("label", label), ("instead", instead(&words.smell, label))],
        );
    }
    let proposable = |label: &str| {
        let permission = label.split(' ').next().unwrap_or(label);
        !words.never_propose.iter().any(|p| p == permission)
    };
    for replayed in &stats.replayed {
        for (label, c) in replayed
            .asks
            .top(usize::MAX)
            .into_iter()
            .filter(|(l, _)| proposable(l))
        {
            push(
                &t.candidate_ask,
                c.n,
                &[("mode", mode_label(&replayed.mode)), ("label", label)],
            );
        }
    }
    for (mode, refused) in &stats.live {
        for (label, c) in refused
            .top(usize::MAX)
            .into_iter()
            .filter(|(l, _)| proposable(l))
        {
            push(
                &t.candidate_live,
                c.n,
                &[("mode", mode_label(mode)), ("label", label)],
            );
        }
    }
    for (label, c) in stats.tools.refusals.top(usize::MAX) {
        if MISREAD
            .iter()
            .any(|m| label.starts_with(&format!("{m} · ")))
        {
            push(&t.candidate_refusal, c.n, &[("label", label)]);
        }
    }
    for (label, c) in stats.tools.retried.top(usize::MAX) {
        push(&t.candidate_retry, c.n, &[("label", label)]);
    }
    for (label, c) in stats.triples.top(3) {
        push(&t.candidate_sequence, c.n, &[("label", label)]);
    }
    all.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    all
}

/// Per run, in the order of `against_rows`: done, turns, calls, bash calls, stand-ins, cost.
fn measures(battery: &Battery, stats: &Stats) -> [f64; 6] {
    let runs: Vec<&Run> = battery
        .runs
        .iter()
        .filter(|r| r.record.error.is_none())
        .collect();
    let n = runs.len().max(1) as f64;
    [
        runs.iter().filter(|r| r.record.check).count() as f64 / n,
        runs.iter().map(|r| f64::from(r.record.turns)).sum::<f64>() / n,
        runs.iter().map(|r| r.calls.len() as f64).sum::<f64>() / n,
        stats.shell.calls as f64 / n,
        stats.shell.smells.total() as f64 / n,
        runs.iter()
            .map(|r| r.record.cost_micros as f64)
            .sum::<f64>()
            / n,
    ]
}

fn counts(columns: &[String], counts: &Counts, top: usize) -> String {
    table(
        columns,
        counts
            .top(top)
            .iter()
            .map(|(k, c)| vec![(*k).to_owned(), c.n.to_string(), example(c)])
            .collect(),
    )
}

fn table(columns: &[String], rows: Vec<Vec<String>>) -> String {
    if rows.is_empty() {
        return "—".to_owned();
    }
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut lines = vec![
        format!(
            "| {} |",
            columns
                .iter()
                .map(|c| cell(c))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        format!("|{}|", vec!["---"; columns.len()].join("|")),
    ];
    for row in rows {
        lines.push(format!(
            "| {} |",
            row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
        ));
    }
    lines.join("\n")
}

fn example(count: &Count) -> String {
    match count.example.as_str() {
        "" => String::new(),
        text => format!("`{}`", shorten::line(text, EXAMPLE_CHARS).replace('`', "'")),
    }
}

fn instead<'a>(smells: &'a [Smell], label: &str) -> &'a str {
    smells
        .iter()
        .find(|s| s.label == label)
        .map_or("", |s| s.instead.as_str())
}

/// A failed reply by its kind, and its HTTP status when it had one.
fn failure(e: &ErrorInfo) -> String {
    match e.status {
        Some(status) => format!("{} {status}", e.kind),
        None => e.kind.clone(),
    }
}

fn mode_label(id: &str) -> &str {
    mode::get(id).map_or(id, |m| m.label.as_str())
}

fn done(runs: &[&Run]) -> String {
    format!(
        "{}/{}",
        runs.iter().filter(|r| r.record.check).count(),
        runs.len()
    )
}

fn dollars(micros: u64) -> String {
    format!("{:.4}", micros as f64 / 1e6)
}

fn one_place(x: f64) -> String {
    format!("{x:.1}")
}

fn short_rev(rev: &str) -> &str {
    rev.get(..7).unwrap_or(rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::fixture;
    use crate::settings;

    #[test]
    fn the_report_ranks_what_the_counts_say() {
        let battery = fixture::battery();
        let text = render(
            &battery,
            Some(&battery),
            &fixture::words(),
            &settings::default_rules(),
        );
        insta::assert_snapshot!(text);
    }
}
