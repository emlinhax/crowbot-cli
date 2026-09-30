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

/// Why a call does not run: what the model is told, and whether the run stops there.
struct Refused {
    why: String,
    stop: bool,
}

impl From<String> for Refused {
    fn from(why: String) -> Self {
        Self { why, stop: false }
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
            Err(Refused::from(model_text::get().rejected.clone()))
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
        let out = match slot {
            Err(refused) => Output::error(refused.why),
            Ok((tool, args)) => {
                let tcx = tool_cx(cx, shared, cancel, &call.id);
                tokio::select! {
                    out = tool.run(args, &tcx) => out,
                    () = cancel.cancelled() => Output::error(model_text::get().cancelled.clone()),
                }
            }
        };
        let done = result(call, out);
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
) -> Result<(Arc<dyn Tool>, Value), Refused> {
    let text = model_text::get();
    if cancel.is_cancelled() {
        return Err(text.cancelled.clone().into());
    }
    let tool = cx.tools.get(&call.name).ok_or_else(|| {
        fill(
            &text.unknown_tool,
            &[("name", &call.name), ("available", &cx.tools.names())],
        )
    })?;
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|e| fill(&text.bad_json, &[("error", &e.to_string())]))?;

    let mut asks = Vec::new();
    if repeats >= limits::get().agent.doom_loop_repeats.value {
        match mode.verdicts.doom_loop {
            DoomLoop::TellModel => {
                return Err(fill(&text.doom_loop, &[("count", &repeats.to_string())]).into());
            }
            DoomLoop::Ask => asks.push(Ask::new(permissions::DOOM_LOOP.name, call.name.clone())),
        }
    }
    let check = tool
        .check(&args, &tool_cx(cx, shared, cancel, &call.id))
        .map_err(|refusal| match refusal {
            Refusal::InvalidArgs(error) => fill(
                &text.invalid_arguments,
                &[("name", &call.name), ("error", &error)],
            ),
            Refusal::Refused(why) => why,
        })?;
    asks.extend(check.asks);

    let policy = Policy {
        rules: &cx.app.settings.permission,
        mode,
        plan_file: &cx.plan_file,
    };
    match policy.decide(&asks) {
        Decision::Allow => Ok((tool.clone(), args)),
        Decision::Deny(what) => Err(fill(&text.denied, &[("what", &what)]).into()),
        Decision::Ask => {
            let prompt = Prompt::Permission {
                tool: call.name.clone(),
                asks: asks.clone(),
                preview: check.preview,
            };
            match prompt::ask(shared, cx.emit, &call.id, prompt, cancel).await {
                Some(Reply::Yes) => Ok((tool.clone(), args)),
                Some(Reply::No { feedback: Some(f) } | Reply::Text(f)) => {
                    Err(fill(&text.rejected_with_feedback, &[("feedback", &f)]).into())
                }
                Some(Reply::No { feedback: None } | Reply::Choice(_)) => Err(Refused {
                    why: text.rejected.clone(),
                    stop: true,
                }),
                Some(Reply::Unavailable) => {
                    Err(fill(&text.non_interactive, &[("what", &gate::describe(&asks))]).into())
                }
                None => Err(text.cancelled.clone().into()),
            }
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
    }
}
