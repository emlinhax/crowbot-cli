//! One reply's tool calls: cleared one by one in the order the model made them (prompts stay
//! in order), then run together, with results kept in call order.

use std::sync::Arc;

use futures_util::future::join_all;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent::doom_loop;
use crate::agent::event::AgentEvent;
use crate::agent::message::{Message, ToolCall, ToolResult};
use crate::agent::model_text;
use crate::agent::prompt::{self, Prompt, Reply};
use crate::agent::run::RunCtx;
use crate::agent::state::Shared;
use crate::limits;
use crate::mode::{DoomLoop, Mode};
use crate::permission::gate::{self, Ask, Decision, Policy};
use crate::text::template::fill;
use crate::tools::{Output, Refusal, Tool, ToolCx, permissions};

pub struct Batch {
    pub results: Vec<ToolResult>,
    /// The user declined without a word: stop the run.
    pub stop: bool,
}

/// A call that may run, and what it was cleared for.
struct Cleared {
    tool: Arc<dyn Tool>,
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

pub async fn run<'a>(
    cx: &'a RunCtx<'a>,
    history: &[Message],
    calls: &'a [ToolCall],
    shared: &'a Shared,
    cancel: &'a CancellationToken,
) -> Batch {
    let mode = shared.mode();

    let mut cleared = Vec::with_capacity(calls.len());
    let mut stop = false;
    for (i, call) in calls.iter().enumerate() {
        let slot = if stop {
            Err(Refused::new("rejected", model_text::get().rejected.clone()))
        } else {
            let repeats = doom_loop::repeats(history, calls, i);
            clear(cx, shared, cancel, mode, repeats, call).await
        };
        if let Err(refused) = &slot {
            stop |= refused.stop;
        }
        cleared.push(slot);
    }

    // Polled in order, so tools start, and results come back, in the order they were called.
    // A refused call gets its start and end too, so a frontend shows every call's fate.
    let results = join_all(cleared.into_iter().zip(calls).map(|(slot, call)| async move {
        (cx.emit)(AgentEvent::ToolStart {
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
                let tcx = tool_cx(cx, shared, cancel, &call.id);
                let out = tokio::select! {
                    out = cleared.tool.run(cleared.args, &tcx) => out,
                    () = cancel.cancelled() => Output::error(model_text::get().cancelled.clone()),
                };
                ToolResult {
                    asks: cleared.asks,
                    ..result(call, out)
                }
            }
        };
        (cx.emit)(AgentEvent::ToolEnd {
            result: done.clone(),
        });
        done
    }))
    .await;
    Batch { results, stop }
}

/// Everything a call must pass before it runs, in order: not cancelled, a known tool, valid
/// JSON, not a loop, the tool's own check, then the permission policy (asking if it says so).
async fn clear<'a>(
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: &'a CancellationToken,
    mode: &Mode,
    repeats: usize,
    call: &'a ToolCall,
) -> Result<Cleared, Refused> {
    let text = model_text::get();
    if cancel.is_cancelled() {
        return Err(Refused::new("cancelled", text.cancelled.clone()));
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
    if repeats >= limits::get().agent.doom_loop_repeats.value {
        match mode.verdicts.doom_loop {
            DoomLoop::TellModel => {
                return Err(Refused::new(
                    "doom_loop",
                    fill(&text.doom_loop, &[("count", &repeats.to_string())]),
                ));
            }
            DoomLoop::Ask => asks.push(Ask::new(permissions::DOOM_LOOP.name, call.name.clone())),
        }
    }
    let check = tool
        .check(&args, &tool_cx(cx, shared, cancel, &call.id))
        .map_err(|refusal| match refusal {
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

    let policy = Policy {
        rules: &cx.app.settings.permission,
        mode,
        plan_file: &cx.plan_file,
    };
    let cleared = |asks| Cleared {
        tool: tool.clone(),
        args,
        asks,
    };
    match policy.decide(&asks) {
        Decision::Allow => Ok(cleared(asks)),
        Decision::Deny(what) => {
            Err(Refused::new("denied", fill(&text.denied, &[("what", &what)])).asked(asks))
        }
        Decision::Ask => {
            let prompt = Prompt::Permission {
                tool: call.name.clone(),
                asks: asks.clone(),
                preview: check.preview,
            };
            let reply = prompt::ask(shared, cx.emit, &call.id, prompt, cancel).await;
            let refused = match reply {
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
                Some(Reply::Unavailable) => Refused::new(
                    "non_interactive",
                    fill(&text.non_interactive, &[("what", &gate::describe(&asks))]),
                ),
                None => Refused::new("cancelled", text.cancelled.clone()),
            };
            Err(refused.asked(asks))
        }
    }
}

fn tool_cx<'a>(
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: &CancellationToken,
    call_id: &'a str,
) -> ToolCx<'a> {
    ToolCx {
        app: cx.app,
        shared,
        emit: cx.emit,
        call_id,
        plan_file: &cx.plan_file,
        cancel: cancel.clone(),
    }
}

/// Every call failed with the same message, e.g. when the reply was cut off mid-arguments.
pub fn fail_all(calls: &[ToolCall], why: &str) -> Batch {
    Batch {
        results: calls
            .iter()
            .map(|c| result(c, Output::error(why)))
            .collect(),
        stop: false,
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
