//! Summarising earlier turns so the conversation keeps within the model's window: everything but a
//! recent tail (`compaction.keep_tokens`) becomes a summary the model writes itself
//! (data/prompts/compact.md), sent ahead of the tail from then on.

use crate::agent::{RunCtx, context, model_text};
use crate::api::chat::{self, Turn};
use crate::config::limits;
use tokio_util::sync::CancellationToken;

use crate::api::error::headline;
use crate::conversation::error::ErrorInfo;
use crate::conversation::message::{Message, Part, Usage};
use crate::data;
use crate::live::event::{AgentEvent, Ended, Outcome};
use crate::live::state::Shared;
use crate::session::entry::Summary;
use crate::session::transcript::Transcript;
use crate::text::template::fill;

/// Why a summary was made, as the file and the frontends record it.
#[derive(Clone, Copy)]
pub struct Why {
    pub auto: bool,
    pub overflow: bool,
}

/// `/compact`: summarise now, keeping `focus` in mind, as a run of its own.
pub async fn run(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    focus: Option<&str>,
) -> Ended {
    let why = Why {
        auto: false,
        overflow: false,
    };
    let cancel = shared.begin_run();
    let outcome = match compact(cx, transcript, shared, &cancel, focus, why).await {
        Ok(_) => Outcome::Done,
        Err(_) => Outcome::Failed,
    };
    let ended = Ended { outcome, turns: 1 };
    (cx.emit)(AgentEvent::RunEnd(ended));
    ended
}

/// Summarises what precedes the recent tail; `Ok(false)` when it all fits in the tail already.
/// The frontends hear it start and end; `cancel` is the run's interrupt.
pub async fn compact(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    cancel: &CancellationToken,
    focus: Option<&str>,
    why: Why,
) -> Result<bool, String> {
    (cx.emit)(AgentEvent::CompactionStart {
        auto: why.auto,
        overflow: why.overflow,
    });
    let from = transcript.summarised();
    let Some(cut) = cut(&transcript.messages, from) else {
        (cx.emit)(AgentEvent::CompactionEnd {
            summarised: 0,
            kept: transcript.messages.len() - from,
            usage: None,
            error: None,
        });
        return Ok(false);
    };
    let done = summarise(
        cx,
        transcript,
        cancel,
        &transcript.messages[from..cut],
        focus,
    )
    .await;
    let (usage, error) = match done {
        Ok((text, usage)) => {
            let summary = Summary {
                summary: text,
                auto: why.auto,
                overflow: why.overflow,
                model: cx.model.id.clone(),
                usage,
            };
            if let Err(unsaved) = transcript.compact(cut, summary) {
                (cx.emit)(AgentEvent::Unsaved {
                    path: unsaved.path.display().to_string(),
                    error: unsaved.error,
                });
            }
            // The mode's instructions may be among what was summarised.
            shared.forget_reminded();
            (usage, None)
        }
        Err(error) => (None, Some(error)),
    };
    (cx.emit)(AgentEvent::CompactionEnd {
        summarised: if error.is_none() { cut - from } else { 0 },
        kept: transcript.messages.len() - cut,
        usage,
        error: error.clone(),
    });
    error.map_or(Ok(true), Err)
}

/// Asks the session's model, with its own system prompt and tools so the request's prefix is
/// the one it has cached, to summarise `older` after the summary already in force.
async fn summarise(
    cx: &RunCtx<'_>,
    transcript: &Transcript,
    cancel: &CancellationToken,
    older: &[Message],
    focus: Option<&str>,
) -> Result<(String, Option<Usage>), String> {
    let (previous, _) = transcript.context();
    let mut messages = context::build(previous, older);
    let focus = focus.map_or_else(String::new, |f| {
        fill(&model_text::get().compact_focus, &[("focus", f)])
    });
    let ask = fill(
        data::raw!("prompts/compact.md").trim_end(),
        &[("focus", &focus)],
    );
    messages.push(Message::User {
        parts: vec![Part::Text { text: ask }],
    });
    let tools = cx.tools.wire();
    let turn = Turn {
        model: cx.model,
        effort: cx.effort,
        system: cx.system,
        messages: &messages,
        tools: &tools,
    };
    // CEILING: nothing stops the model calling a tool here (no `tool_choice`); a reply with no
    // text fails the compaction. Upgrade: send `tool_choice: "none"` once crowbot passes it on.
    let reply = chat::stream(&cx.app.api, &turn, cancel, &mut |_| {}).await;
    if let Some(error) = &reply.error {
        return Err(headline(error));
    }
    let text = Part::text_of(&reply.parts);
    if text.trim().is_empty() {
        let empty = ErrorInfo::local("summary_empty", "the reply held no text");
        return Err(headline(&empty));
    }
    Ok((text, reply.usage))
}

/// Where the word-for-word tail starts: the latest prompt or reply from which the messages to
/// the end reach `compaction.keep_tokens`, so a call is never kept without its result. `None`
/// when everything since `from` fits.
// CEILING: tokens are estimated as bytes / `compaction.bytes_per_token`, with no tokenizer.
fn cut(messages: &[Message], from: usize) -> Option<usize> {
    let limits = &limits::get().compaction;
    let budget = limits.keep_tokens.value * limits.bytes_per_token.value;
    let mut bytes = 0;
    for i in (from..messages.len()).rev() {
        bytes += serde_json::to_string(&messages[i]).map_or(0, |s| s.len());
        if bytes >= budget && !matches!(messages[i], Message::Tool(_)) {
            return (i > from).then_some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::{Assistant, Finish, ToolResult};

    fn big(role: &str) -> Message {
        let text = "x".repeat(limits::get().compaction.keep_tokens.value * 2);
        match role {
            "user" => Message::user_text(text),
            "tool" => Message::Tool(ToolResult {
                call_id: "c".into(),
                name: "read".into(),
                content: text,
                is_error: false,
                details: None,
                asks: Vec::new(),
                refused: None,
            }),
            _ => Message::Assistant(Assistant::stub(Finish::Done, vec![Part::Text { text }])),
        }
    }

    #[test]
    fn the_tail_starts_at_a_prompt_or_reply_once_it_holds_enough() {
        let small = Message::user_text("hi");
        // Everything fits: nothing to summarise.
        assert_eq!(cut(&[small.clone(), small.clone()], 0), None);
        // The last two are a reply and its big tool result: the tail starts at the reply.
        let messages = [big("user"), big("assistant"), big("assistant"), big("tool")];
        assert_eq!(cut(&messages, 0), Some(2));
        // Not before what an earlier summary already covers.
        assert_eq!(cut(&messages, 2), None);
    }
}
