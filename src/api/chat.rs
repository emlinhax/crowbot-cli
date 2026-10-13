//! One model turn: send the conversation, stream the reply, retry what is safe to retry.
//! Never fails: every outcome, including errors and cancellation, is an `Assistant` message.

use std::time::Duration;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::api::assemble::{Assembler, Delta};
use crate::api::error::ApiError;
use crate::api::models::Model;
use crate::api::sse;
use crate::api::wire::{self, WireUsage};
use crate::api::{Api, Call, retry};
use crate::config::limits;
use crate::conversation::error::ErrorInfo;
use crate::conversation::message::{Assistant, Finish, Message, Usage};
use crate::io;

pub struct Turn<'a> {
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [serde_json::Value],
}

impl Turn<'_> {
    /// The effort sent and recorded: none for a model that does not reason.
    pub fn effort(&self) -> Option<&str> {
        self.model.effort(self.effort)
    }
}

pub enum Event<'a> {
    Delta(Delta<'a>),
    Retry {
        attempt: u32,
        delay: Duration,
        error: &'a ErrorInfo,
    },
}

pub async fn stream(
    api: &Api,
    turn: &Turn<'_>,
    cancel: &CancellationToken,
    on: &mut (dyn FnMut(Event<'_>) + Send),
) -> Assistant {
    let body = serde_json::to_value(wire::Request::from(turn)).expect("chat requests serialize");
    let mut attempt = 1;
    loop {
        let outcome = attempt_once(api, turn, &body, cancel, on).await;
        if !outcome.emitted
            && let Some(error) = &outcome.message.error
            && let Some(delay) = retry::delay(error, outcome.retry_after, attempt)
        {
            on(Event::Retry {
                attempt,
                delay,
                error,
            });
            tokio::select! {
                () = io::clock::sleep(delay) => {}
                () = cancel.cancelled() => return reply(turn, Finish::Aborted),
            }
            attempt += 1;
            continue;
        }
        return outcome.message;
    }
}

struct Outcome {
    message: Assistant,
    emitted: bool,
    retry_after: Option<Duration>,
}

async fn attempt_once(
    api: &Api,
    turn: &Turn<'_>,
    body: &serde_json::Value,
    cancel: &CancellationToken,
    on: &mut (dyn FnMut(Event<'_>) + Send),
) -> Outcome {
    let limits = &limits::get().chat;
    let call = Call {
        body: Some(body),
        timeout: Some(limits.headers_timeout_secs.secs()),
        ..Call::default()
    };
    let opened = tokio::select! {
        r = api.open("chat", call) => r,
        () = cancel.cancelled() => return Outcome {
            message: reply(turn, Finish::Aborted),
            emitted: false,
            retry_after: None,
        },
    };
    let resp = match opened {
        Ok(resp) => resp,
        Err(ApiError { info, retry_after }) => {
            return Outcome {
                message: failed(turn, info),
                emitted: false,
                retry_after,
            };
        }
    };

    let events = sse::events(
        resp.body,
        limits.idle_timeout_secs.secs(),
        limits.stall_timeout_secs.secs(),
        limits.max_event_bytes.value,
    );
    let mut events = std::pin::pin!(events);
    let mut asm = Assembler::default();
    let mut cut = None;
    let mut aborted = false;
    loop {
        let next = tokio::select! {
            next = events.next() => next,
            () = cancel.cancelled() => { aborted = true; break; }
        };
        match next {
            None => break,
            Some(Err(error)) => {
                cut = Some(error);
                break;
            }
            Some(Ok(data)) => match serde_json::from_str(&data) {
                Ok(chunk) => asm.apply(chunk, &mut |d| on(Event::Delta(d))),
                Err(e) => {
                    cut = Some(ErrorInfo::local("bad_stream", e.to_string()));
                    break;
                }
            },
        }
    }

    let emitted = asm.emitted;
    let request_id = resp.head.request_id.clone();
    let done = asm.finish(cut);
    let (finish, error) = if aborted {
        (Finish::Aborted, None)
    } else {
        (
            done.finish,
            done.error.map(|e| with_request_id(e, &request_id)),
        )
    };
    let usage = done.usage.map(|u| cost(turn.model, &u));
    Outcome {
        message: Assistant {
            parts: done.parts,
            usage,
            error,
            request_id,
            ..reply(turn, finish)
        },
        emitted,
        retry_after: None,
    }
}

fn cost(model: &Model, usage: &WireUsage) -> Usage {
    let cached = usage.cached().min(usage.prompt_tokens);
    let uncached = usage.prompt_tokens - cached;
    Usage {
        input: usage.prompt_tokens,
        cached,
        output: usage.completion_tokens,
        reasoning: usage.reasoning(),
        cost_micros: model
            .pricing
            .cost_micros(uncached, cached, usage.completion_tokens),
    }
}

fn with_request_id(mut error: ErrorInfo, request_id: &Option<String>) -> ErrorInfo {
    if error.request_id.is_none() {
        error.request_id.clone_from(request_id);
    }
    error
}

/// A reply with nothing in it, as a turn records one.
fn reply(turn: &Turn<'_>, finish: Finish) -> Assistant {
    Assistant {
        parts: Vec::new(),
        model: turn.model.id.clone(),
        effort: turn.effort().map(str::to_owned),
        usage: None,
        finish,
        error: None,
        request_id: None,
    }
}

/// A reply that never came, recording why.
fn failed(turn: &Turn<'_>, info: ErrorInfo) -> Assistant {
    Assistant {
        request_id: info.request_id.clone(),
        error: Some(info),
        ..reply(turn, Finish::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::model;

    #[test]
    fn cost_splits_cached_and_uncached_input() {
        let usage: WireUsage = serde_json::from_str(
            r#"{"prompt_tokens":1000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":400}}"#,
        )
        .unwrap();
        let u = cost(&model(true), &usage);
        // 600 * 6 + 400 * 0.6 + 100 * 10 micro-dollars
        assert_eq!(u.cost_micros, 3600 + 240 + 1000);
        assert_eq!(u.cached, 400);
    }

    #[test]
    fn effort_is_sent_and_recorded_only_for_reasoning_models() {
        let m = model(false);
        let turn = Turn {
            model: &m,
            effort: Some("high"),
            system: "sys",
            messages: &[],
            tools: &[],
        };
        let recorded = |turn: &Turn<'_>| reply(turn, Finish::Done).effort;
        let sent = |turn: &Turn<'_>| wire::Request::from(turn).reasoning_effort;
        assert_eq!((sent(&turn), recorded(&turn)), (None, None));
        let m = model(true);
        let turn = Turn { model: &m, ..turn };
        assert_eq!(sent(&turn).as_deref(), Some("high"));
        assert_eq!(recorded(&turn).as_deref(), Some("high"));
    }
}
