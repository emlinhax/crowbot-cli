//! One model turn: send the conversation, stream the reply, retry what is safe to retry.
//! Never fails: every outcome, including errors and cancellation, is an `Assistant` message.

use std::time::Duration;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent::message::{Assistant, Finish, Message, Usage};
use crate::api::assemble::{Assembler, Delta};
use crate::api::error::{ApiError, ErrorInfo};
use crate::api::models::Model;
use crate::api::sse::{self, Frame};
use crate::api::wire::{self, Replay, StreamOptions, WireMessage, WireUsage};
use crate::api::{Api, Call, retry};
use crate::limits;

pub struct Turn<'a> {
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [serde_json::Value],
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
    let body = serde_json::to_value(request(turn)).expect("chat requests serialize");
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
                () = tokio::time::sleep(delay) => {}
                () = cancel.cancelled() => return message(turn, Vec::new(), Finish::Aborted, None, None, None),
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
        ..Call::default()
    };
    let opened = tokio::select! {
        r = api.open("chat", call, limits.headers_timeout_secs.secs()) => r,
        () = cancel.cancelled() => return Outcome {
            message: message(turn, Vec::new(), Finish::Aborted, None, None, None),
            emitted: false,
            retry_after: None,
        },
    };
    let mut resp = match opened {
        Ok(resp) => resp,
        Err(ApiError { info, retry_after }) => {
            let request_id = info.request_id.clone();
            return Outcome {
                message: message(
                    turn,
                    Vec::new(),
                    Finish::Error,
                    None,
                    Some(info),
                    request_id,
                ),
                emitted: false,
                retry_after,
            };
        }
    };

    let mut parser = sse::Parser::default();
    let mut asm = Assembler::default();
    let mut cut = None;
    let mut aborted = false;
    let idle = limits.idle_timeout_secs.secs();
    'read: loop {
        let next = tokio::select! {
            next = tokio::time::timeout(idle, resp.body.next()) => next,
            () = cancel.cancelled() => { aborted = true; break 'read; }
        };
        let ended = matches!(next, Ok(None));
        let frames = match next {
            Err(_) => {
                cut = Some(ErrorInfo::local(
                    "idle_timeout",
                    format!("no data for {}s", idle.as_secs()),
                ));
                break;
            }
            Ok(None) => parser.finish(),
            Ok(Some(Err(e))) => {
                cut = Some(ApiError::from(e).info);
                break;
            }
            Ok(Some(Ok(bytes))) => parser.feed(&bytes),
        };
        for frame in frames {
            match frame {
                Frame::Comment => {}
                Frame::Data(data) if data == "[DONE]" => break 'read,
                Frame::Data(data) => match serde_json::from_str(&data) {
                    Ok(chunk) => asm.apply(chunk, &mut |d| on(Event::Delta(d))),
                    Err(e) => {
                        cut = Some(ErrorInfo::local("bad_stream", e.to_string()));
                        break 'read;
                    }
                },
            }
        }
        if ended {
            break;
        }
    }

    let emitted = asm.emitted;
    let request_id = resp.request_id.clone();
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
        message: message(turn, done.parts, finish, usage, error, request_id),
        emitted,
        retry_after: None,
    }
}

fn request(turn: &Turn<'_>) -> wire::Request {
    let replay = Replay {
        model: &turn.model.id,
        reasoning: turn.model.capabilities.reasoning,
    };
    let mut messages = vec![WireMessage::System {
        content: turn.system.to_owned(),
    }];
    messages.extend(
        turn.messages
            .iter()
            .map(|m| WireMessage::from_message(m, &replay)),
    );
    wire::Request {
        model: turn.model.id.clone(),
        messages,
        tools: turn.tools.to_vec(),
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        reasoning_effort: turn
            .effort
            .filter(|_| turn.model.capabilities.reasoning)
            .map(str::to_owned),
    }
}

/// Tokens times USD-per-million is exactly micro-dollars.
fn cost(model: &Model, usage: &WireUsage) -> Usage {
    let cached = usage.cached().min(usage.prompt_tokens);
    let p = &model.pricing;
    let micros = (usage.prompt_tokens - cached) as f64 * p.input_per_1m_usd
        + cached as f64 * p.cached_input_per_1m_usd
        + usage.completion_tokens as f64 * p.output_per_1m_usd;
    Usage {
        input: usage.prompt_tokens,
        cached,
        output: usage.completion_tokens,
        reasoning: usage.reasoning(),
        cost_micros: micros.round() as u64,
    }
}

fn with_request_id(mut error: ErrorInfo, request_id: &Option<String>) -> ErrorInfo {
    if error.request_id.is_none() {
        error.request_id.clone_from(request_id);
    }
    error
}

fn message(
    turn: &Turn<'_>,
    parts: Vec<crate::agent::message::Part>,
    finish: Finish,
    usage: Option<Usage>,
    error: Option<ErrorInfo>,
    request_id: Option<String>,
) -> Assistant {
    Assistant {
        parts,
        model: turn.model.id.clone(),
        effort: turn.effort.map(str::to_owned),
        usage,
        finish,
        error,
        request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Capabilities, Pricing};

    fn model(reasoning: bool) -> Model {
        Model {
            id: "m".into(),
            display_name: String::new(),
            description: String::new(),
            context_window: 1000,
            max_output_tokens: 100,
            pricing: Pricing {
                input_per_1m_usd: 6.0,
                cached_input_per_1m_usd: 0.6,
                output_per_1m_usd: 10.0,
            },
            capabilities: Capabilities {
                reasoning,
                ..Capabilities::default()
            },
        }
    }

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
    fn effort_is_sent_only_to_reasoning_models() {
        let m = model(false);
        let turn = Turn {
            model: &m,
            effort: Some("high"),
            system: "sys",
            messages: &[],
            tools: &[],
        };
        assert!(request(&turn).reasoning_effort.is_none());
        let m = model(true);
        let turn = Turn { model: &m, ..turn };
        assert_eq!(request(&turn).reasoning_effort.as_deref(), Some("high"));
    }
}
