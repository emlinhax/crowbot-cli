//! One reply's tool calls: cleared one by one in the order the model made them (prompts stay
//! in order), then run together, with results kept in call order.

use std::sync::Arc;

use futures_util::future::join_all;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent::doom_loop;
use crate::agent::event::AgentEvent;
use crate::agent::message::{Message, ToolCall, ToolResult};
use crate::agent::model_text::{self, fill};
use crate::agent::run::RunCtx;
use crate::agent::state::{Reply, Shared};
use crate::limits;
use crate::mode::DoomLoop;
use crate::permission::gate::{self, Ask, Decision, Policy};
use crate::tools::{Output, Tool, ToolCx};

pub struct Batch {
    pub results: Vec<ToolResult>,
    /// The user declined without a word: stop the run.
    pub stop: bool,
}

enum Slot {
    Done(ToolResult),
    Run(Arc<dyn Tool>, Value),
}

pub async fn run(
    cx: &RunCtx<'_>,
    history: &[Message],
    calls: &[ToolCall],
    shared: &Shared,
    cancel: &CancellationToken,
) -> Batch {
    let text = model_text::get();
    let mode = shared.mode();
    let tcx = ToolCx {
        app: cx.app,
        files: &shared.files,
        cancel: cancel.clone(),
    };
    let earlier: Vec<&ToolCall> = history
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.tool_calls()),
            _ => None,
        })
        .flatten()
        .collect();

    let mut slots = Vec::with_capacity(calls.len());
    let mut stop = false;
    for (i, call) in calls.iter().enumerate() {
        let fail = |content: String| Slot::Done(result(call, Output::error(content)));
        if stop {
            slots.push(fail(text.rejected.clone()));
            continue;
        }
        if cancel.is_cancelled() {
            slots.push(fail(text.cancelled.clone()));
            continue;
        }
        let Some(tool) = cx.tools.get(&call.name) else {
            slots.push(fail(fill(
                &text.unknown_tool,
                &[("name", &call.name), ("available", &cx.tools.names())],
            )));
            continue;
        };
        let args: Value = match serde_json::from_str(&call.arguments) {
            Ok(args) => args,
            Err(e) => {
                slots.push(fail(fill(&text.bad_json, &[("error", &e.to_string())])));
                continue;
            }
        };

        let mut asks = Vec::new();
        let count = doom_loop::repeats(earlier.iter().copied().chain(&calls[..i]), call);
        if count >= limits::get().agent.doom_loop_repeats.value {
            match mode.verdicts.doom_loop {
                DoomLoop::TellModel => {
                    slots.push(fail(fill(
                        &text.doom_loop,
                        &[("count", &count.to_string())],
                    )));
                    continue;
                }
                DoomLoop::Ask => asks.push(Ask::new("doom_loop", call.name.clone())),
            }
        }
        match tool.check(&args, &tcx) {
            Ok(check) => asks.extend(check.asks),
            Err(refusal) => {
                slots.push(fail(fill(
                    &text.invalid_arguments,
                    &[("name", &call.name), ("error", &refusal.to_string())],
                )));
                continue;
            }
        }

        let approved = shared.approved();
        let policy = Policy {
            rules: &cx.app.settings.permission,
            approved: &approved,
            mode,
            plan_file: &cx.plan_file,
        };
        let slot = match policy.decide(&asks) {
            Decision::Allow => Slot::Run(tool.clone(), args),
            Decision::Deny(what) => fail(fill(&text.denied, &[("what", &what)])),
            Decision::Ask => match ask_user(cx, shared, call, &asks, cancel).await {
                Some(Reply::Once) => Slot::Run(tool.clone(), args),
                Some(Reply::Always) => {
                    shared.approve(asks.iter().flat_map(Ask::always_rules));
                    Slot::Run(tool.clone(), args)
                }
                Some(Reply::Reject { feedback: Some(f) }) => {
                    fail(fill(&text.rejected_with_feedback, &[("feedback", &f)]))
                }
                Some(Reply::Reject { feedback: None }) => {
                    stop = true;
                    fail(text.rejected.clone())
                }
                Some(Reply::Unavailable) => fail(fill(
                    &text.non_interactive,
                    &[("what", &gate::describe(&asks))],
                )),
                None => fail(text.cancelled.clone()),
            },
        };
        slots.push(slot);
    }

    let running = slots.iter().enumerate().filter_map(|(i, slot)| match slot {
        Slot::Run(tool, args) => Some((i, tool.clone(), args.clone())),
        Slot::Done(_) => None,
    });
    let finished = join_all(running.map(|(i, tool, args)| {
        let (call, tcx) = (&calls[i], &tcx);
        async move {
            (cx.emit)(AgentEvent::ToolStart {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let out = tokio::select! {
                out = tool.run(args, tcx) => out,
                () = cancel.cancelled() => Output::error(model_text::get().cancelled.clone()),
            };
            let done = result(call, out);
            (cx.emit)(AgentEvent::ToolEnd {
                result: done.clone(),
            });
            (i, done)
        }
    }))
    .await;
    for (i, done) in finished {
        slots[i] = Slot::Done(done);
    }
    let results = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Done(done) => done,
            Slot::Run(..) => unreachable!("every approved call ran"),
        })
        .collect();
    Batch { results, stop }
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

async fn ask_user(
    cx: &RunCtx<'_>,
    shared: &Shared,
    call: &ToolCall,
    asks: &[Ask],
    cancel: &CancellationToken,
) -> Option<Reply> {
    let (id, reply) = shared.register_ask();
    (cx.emit)(AgentEvent::Ask {
        id,
        call_id: call.id.clone(),
        tool: call.name.clone(),
        asks: asks.to_vec(),
    });
    tokio::select! {
        reply = reply => Some(reply.unwrap_or(Reply::Unavailable)),
        () = cancel.cancelled() => None,
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
