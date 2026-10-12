//! A battery's directory read back: its results and, for each run, the tool calls its session
//! made, each paired with its result.

use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::Value;

use super::record::Record;
use crate::agent::message::{Finish, Message, ToolResult, Usage};
use crate::api::error::ErrorInfo;
use crate::io;
use crate::session::reader;

pub struct Battery {
    /// The directory's name: when the battery started.
    pub name: String,
    pub meta: Meta,
    pub runs: Vec<Run>,
}

/// battery.json, written as the battery starts.
#[derive(Default, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub crowbot: String,
}

pub struct Run {
    pub record: Record,
    /// In the order the model made them.
    pub calls: Vec<Call>,
    pub usage: Usage,
    /// Why the last reply failed (a server error, a dropped stream), when it did.
    pub failed: Option<ErrorInfo>,
}

pub struct Call {
    pub name: String,
    /// As the model wrote them, so identical calls compare equal.
    pub arguments: String,
    /// Parsed; `Null` when they were not JSON.
    pub args: Value,
    /// `None` when the run stopped before the call was answered.
    pub result: Option<ToolResult>,
}

pub fn load(dir: &Path) -> anyhow::Result<Battery> {
    let results = io::fs::read_string(&dir.join("results.jsonl"))?
        .with_context(|| format!("no results.jsonl in {}", dir.display()))?;
    let meta = io::fs::read_string(&dir.join("battery.json"))?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut runs = Vec::new();
    for (i, line) in results.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Record =
            serde_json::from_str(line).with_context(|| format!("results.jsonl line {}", i + 1))?;
        let session = match record.dir.as_str() {
            "" => None,
            run => io::fs::read_string(&dir.join(run).join("session.jsonl"))?,
        };
        let messages = session.as_deref().map(reader::messages).unwrap_or_default();
        runs.push(Run::new(record, &messages));
    }
    if runs.is_empty() {
        bail!("{} records no runs", dir.display());
    }
    runs.sort_by(|a, b| (&a.record.task, a.record.rep).cmp(&(&b.record.task, b.record.rep)));
    Ok(Battery {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        meta,
        runs,
    })
}

impl Run {
    pub fn new(record: Record, messages: &[Message]) -> Self {
        let mut usage = Usage::default();
        let mut failed = None;
        let mut calls: Vec<Call> = Vec::new();
        // Results follow their reply's calls; ids are matched among the calls still open, since
        // some models reuse an id from one reply to the next.
        let mut open: VecDeque<(String, usize)> = VecDeque::new();
        for message in messages {
            match message {
                Message::Assistant(reply) => {
                    failed = match reply.finish {
                        Finish::Error => Some(
                            reply
                                .error
                                .clone()
                                .unwrap_or_else(|| ErrorInfo::local("unknown", "")),
                        ),
                        _ => None,
                    };
                    if let Some(u) = reply.usage {
                        usage.input += u.input;
                        usage.cached += u.cached;
                        usage.output += u.output;
                        usage.reasoning += u.reasoning;
                        usage.cost_micros += u.cost_micros;
                    }
                    for call in reply.tool_calls() {
                        open.push_back((call.id.clone(), calls.len()));
                        calls.push(Call {
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            args: serde_json::from_str(&call.arguments).unwrap_or(Value::Null),
                            result: None,
                        });
                    }
                }
                Message::Tool(result) => {
                    if let Some(at) = open.iter().position(|(id, _)| *id == result.call_id) {
                        let (_, index) = open.remove(at).expect("found");
                        calls[index].result = Some(result.clone());
                    }
                }
                Message::User { .. } => {}
            }
        }
        Self {
            record,
            calls,
            usage,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::battery::fixture;

    #[test]
    fn calls_pair_with_their_results_even_when_an_id_comes_again() {
        let battery = fixture::battery();
        assert_eq!(battery.meta.model, "m");
        let tasks: Vec<&str> = battery
            .runs
            .iter()
            .map(|r| r.record.task.as_str())
            .collect();
        assert_eq!(tasks, ["broken", "fix-it", "flaky", "plan-it"]);
        let fix = &battery.runs[1];
        let names: Vec<&str> = fix.calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["bash", "grep", "bash", "bash", "edit"]);
        let contents: Vec<&str> = fix
            .calls
            .iter()
            .map(|c| c.result.as_ref().unwrap().content.as_str())
            .collect();
        assert_eq!(contents[2..4], ["test failed", "ok"]);
        assert_eq!((fix.usage.input, fix.usage.cost_micros), (4000, 3500));
        assert!(battery.runs[0].calls.is_empty());
        let failed = battery.runs[2].failed.as_ref().unwrap();
        assert_eq!(
            (failed.kind.as_str(), failed.status),
            ("http_error", Some(502))
        );
        assert!(battery.runs[1].failed.is_none());
    }
}
