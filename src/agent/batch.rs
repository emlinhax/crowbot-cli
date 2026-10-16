//! One reply's tool calls: cleared one by one in the order the model made them (prompts stay
//! in order), then run together, with results kept in call order.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::join_all;
use serde_json::Value;

use crate::agent::Run;
use crate::agent::doom_loop;
use crate::agent::model_text;
use crate::conversation::message::{Message, ToolCall, ToolResult};
use crate::live::event::AgentEvent;
use crate::live::prompt::{Prompt, Reply};
use crate::mode::{self, DoomLoop};
use crate::permission::catalog;
use crate::permission::gate::{self, Ask, Decision};
use crate::permission::rule::Rule;
use crate::text::template::fill;
use crate::tools::{self, AnyTool, Output, Refusal, ToolCx};

pub struct Batch {
    pub results: Vec<ToolResult>,
    /// The user declined without a word: stop the run.
    pub stop: bool,
}

/// A call that may run, and what it was cleared for.
struct Cleared {
    tool: Arc<dyn AnyTool>,
    args: Value,
    asks: Vec<Ask>,
}

/// Why a call does not run: what the model is told, and whether the run stops there.
struct Refused {
    why: String,
    /// Recorded with the result; see `ToolResult::refused`.
    kind: &'static str,
    /// What it needed permission for, when it got as far as asking.
    asks: Vec<Ask>,
    stop: bool,
}

impl Refused {
    fn new(kind: &'static str, why: String) -> Self {
        Self {
            why,
            kind,
            asks: Vec::new(),
            stop: false,
        }
    }

    fn asked(self, asks: Vec<Ask>) -> Self {
        Self { asks, ..self }
    }
}

/// A call cleared to run, or why it does not.
type Slot = Result<Cleared, Refused>;

/// Each mode's locks, filled once a batch for the mode in force at each call.
type Locks = HashMap<&'static str, Vec<Rule>>;

pub(super) async fn run<'a>(run: &'a Run<'a>, history: &[Message], calls: &'a [ToolCall]) -> Batch {
    let mut cleared = Vec::with_capacity(calls.len());
    let mut locks = Locks::new();
    let mut stop = false;
    for (i, call) in calls.iter().enumerate() {
        let slot = if stop {
            Err(Refused::new("rejected", model_text::get().rejected.clone()))
        } else {
            let looping = doom_loop::looping(history, calls, i);
            clear(run, &mut locks, looping, call).await
        };
        if let Err(refused) = &slot {
            stop |= refused.stop;
        }
        cleared.push(slot);
    }
    let changes = cleared.iter().flatten().any(|c| {
        c.asks
            .iter()
            .any(|a| catalog::changes_things(&a.permission))
    });
    if changes && run.before.get().is_none() {
        snapshot(run).await;
    }
    Batch {
        results: settle(run, calls, cleared).await,
        stop,
    }
}

/// Keeps the files as they are, before the first call that may change them runs. When the
/// shadow fails here, the frontend hears it once.
async fn snapshot(run: &Run<'_>) {
    let shadow = run.cx.shadow;
    let was_on = shadow.off().is_none();
    match shadow.take().await {
        Some(tree) => {
            let _ = run.before.set(tree);
        }
        None => {
            if let Some(why) = shadow.off().filter(|_| was_on) {
                (run.cx.emit)(AgentEvent::SnapshotOff { why });
            }
        }
    }
}

/// None of the calls run, each refused with `why`, recorded as `kind`; they still get their start
/// and end.
pub(super) async fn refuse_all<'a>(
    run: &'a Run<'a>,
    calls: &'a [ToolCall],
    kind: &'static str,
    why: &str,
) -> Batch {
    let slots = calls
        .iter()
        .map(|_| Err(Refused::new(kind, why.to_owned())))
        .collect();
    Batch {
        results: settle(run, calls, slots).await,
        stop: false,
    }
}

/// Polled in order, so tools start, and results come back, in the order they were called.
/// A refused call gets its start and end too, so a frontend shows every call's fate.
async fn settle<'a>(run: &'a Run<'a>, calls: &'a [ToolCall], slots: Vec<Slot>) -> Vec<ToolResult> {
    join_all(slots.into_iter().zip(calls).map(|(slot, call)| async move {
        (run.cx.emit)(AgentEvent::ToolStart {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
        let done = match slot {
            Err(refused) => ToolResult {
                asks: refused.asks,
                refused: Some(refused.kind.to_owned()),
                ..result(call, Output::error(refused.why))
            },
            Ok(cleared) => {
                let tcx = tool_cx(run, &call.id);
                let out = tokio::select! {
                    out = cleared.tool.run(cleared.args, &tcx) => out,
                    () = run.cancel.cancelled() => Output::cancelled(),
                };
                ToolResult {
                    asks: cleared.asks,
                    ..result(call, out)
                }
            }
        };
        (run.cx.emit)(AgentEvent::ToolEnd {
            result: done.clone(),
        });
        done
    }))
    .await
}

/// Everything a call must pass before it runs, in order: not cancelled, a known tool, valid
/// JSON, not a loop, the tool's own check, then the permission policy (asking if it says so).
/// The mode is read per call: a switch at one call's prompt governs the calls after it.
async fn clear<'a>(
    run: &'a Run<'a>,
    locks: &mut Locks,
    looping: Option<usize>,
    call: &'a ToolCall,
) -> Slot {
    let (cx, shared) = (run.cx, run.shared);
    let mode = shared.mode();
    let text = model_text::get();
    if run.cancel.is_cancelled() {
        return Err(Refused::new("cancelled", tools::text().cancelled.clone()));
    }
    let tool = cx.tools.get(&call.name).ok_or_else(|| {
        Refused::new(
            "unknown_tool",
            fill(
                &text.unknown_tool,
                &[("name", &call.name), ("available", &cx.tools.names())],
            ),
        )
    })?;
    let args: Value = serde_json::from_str(&call.arguments).map_err(|e| {
        Refused::new(
            "bad_json",
            fill(&text.bad_json, &[("error", &e.to_string())]),
        )
    })?;

    let mut asks = Vec::new();
    if let Some(count) = looping {
        match mode.doom_loop {
            DoomLoop::TellModel => {
                return Err(Refused::new(
                    "doom_loop",
                    fill(&text.doom_loop, &[("count", &count.to_string())]),
                ));
            }
            DoomLoop::Ask => asks.push(Ask::new(catalog::DOOM_LOOP.name, call.name.clone())),
        }
    }
    let tcx = tool_cx(run, &call.id);
    let check = tool.check(&args, &tcx).map_err(|refusal| match refusal {
        Refusal::InvalidArgs(error) => Refused::new(
            "invalid_arguments",
            fill(
                &text.invalid_arguments,
                &[("name", &call.name), ("error", &error)],
            ),
        ),
        Refusal::Refused(why) => Refused::new("check", why),
    })?;
    asks.extend(check.asks);

    let lock = locks
        .entry(mode.id.as_str())
        .or_insert_with(|| mode.locks(&run.plan_file));
    let policy = mode.policy(&cx.app.settings.permission, lock);
    let decision = match policy.decide(&asks) {
        Decision::Allow if check.confirm => Decision::Ask,
        decision => decision,
    };
    let cleared = |asks| Cleared {
        tool: tool.clone(),
        args,
        asks,
    };
    match decision {
        Decision::Allow => Ok(cleared(asks)),
        Decision::Deny(what) => {
            Err(Refused::new("denied", fill(&text.denied, &[("what", &what)])).asked(asks))
        }
        Decision::Ask => {
            let prompt = Prompt::Permission {
                tool: call.name.clone(),
                asks: asks.clone(),
                preview: check.preview,
                confirm: check.confirm,
            };
            let refused = match tcx.ask(prompt).await {
                Some(Reply::Yes) => return Ok(cleared(asks)),
                Some(Reply::No { feedback: Some(f) } | Reply::Text(f)) => Refused::new(
                    "rejected_with_feedback",
                    fill(&text.rejected_with_feedback, &[("feedback", &f)]),
                ),
                Some(Reply::No { feedback: None } | Reply::Choice(_) | Reply::Answers(_)) => {
                    Refused {
                        stop: true,
                        ..Refused::new("rejected", text.rejected.clone())
                    }
                }
                Some(Reply::Unavailable) => {
                    let (kind, why) = if check.confirm {
                        ("unconfirmed", &text.unconfirmed)
                    } else {
                        ("non_interactive", &text.non_interactive)
                    };
                    let mode = &mode::unattended().id;
                    let what = gate::describe(&asks);
                    Refused::new(kind, fill(why, &[("what", &what), ("mode", mode)]))
                }
                None => Refused::new("cancelled", tools::text().cancelled.clone()),
            };
            Err(refused.asked(asks))
        }
    }
}

fn tool_cx<'a>(run: &'a Run<'a>, call_id: &'a str) -> ToolCx<'a> {
    ToolCx {
        app: run.cx.app,
        shared: run.shared,
        emit: run.cx.emit,
        call_id,
        plan_file: &run.plan_file,
        cancel: run.cancel.clone(),
        run: Some(run),
    }
}

fn result(call: &ToolCall, out: Output) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: out.content,
        is_error: out.is_error,
        details: out.details,
        asks: Vec::new(),
        refused: None,
    }
}
