//! What the model is sent: the transcript, with interrupted turns repaired so the request is one
//! crowbot's vendors accept (every tool call answered, no half-written calls).

use std::collections::HashSet;

use crate::agent::message::{Assistant, Finish, Message, Part, ToolResult};
use crate::agent::model_text;

pub fn build(messages: &[Message]) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    // Calls from the latest assistant turn still waiting for a result.
    let mut open: Vec<(String, String)> = Vec::new();
    // Calls from cut-off turns: dropped, so their results must go too.
    let mut dropped: HashSet<String> = HashSet::new();

    for message in messages {
        match message {
            Message::Assistant(a) if matches!(a.finish, Finish::Error | Finish::Aborted) => {
                answer_open(&mut out, &mut open);
                dropped.extend(a.tool_calls().map(|c| c.id.clone()));
                if let Some(kept) = completed_text(a) {
                    out.push(Message::Assistant(kept));
                }
            }
            Message::Assistant(a) => {
                answer_open(&mut out, &mut open);
                open = a
                    .tool_calls()
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                out.push(message.clone());
            }
            Message::Tool(result) => {
                if dropped.contains(&result.call_id) {
                    continue;
                }
                open.retain(|(id, _)| *id != result.call_id);
                out.push(message.clone());
            }
            Message::User { .. } => {
                answer_open(&mut out, &mut open);
                out.push(message.clone());
            }
        }
    }
    answer_open(&mut out, &mut open);
    out
}

fn answer_open(out: &mut Vec<Message>, open: &mut Vec<(String, String)>) {
    for (call_id, name) in open.drain(..) {
        out.push(Message::Tool(ToolResult {
            call_id,
            name,
            content: model_text::get().no_result.clone(),
            is_error: true,
            details: None,
        }));
    }
}

/// A cut-off turn keeps only its finished prose; a partial call or trace is not worth replaying.
fn completed_text(a: &Assistant) -> Option<Assistant> {
    let parts: Vec<Part> = a
        .parts
        .iter()
        .filter(|p| matches!(p, Part::Text { text } if !text.trim().is_empty()))
        .cloned()
        .collect();
    (!parts.is_empty()).then(|| Assistant {
        parts,
        finish: Finish::Done,
        ..a.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::ToolCall;

    fn assistant(finish: Finish, parts: Vec<Part>) -> Message {
        Message::Assistant(Assistant {
            parts,
            model: "m".into(),
            effort: None,
            usage: None,
            finish,
            error: None,
            request_id: None,
        })
    }

    fn call(id: &str) -> Part {
        Part::ToolCall(ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: "{}".into(),
        })
    }

    fn result(id: &str) -> Message {
        Message::Tool(ToolResult {
            call_id: id.into(),
            name: "read".into(),
            content: "ok".into(),
            is_error: false,
            details: None,
        })
    }

    fn roles(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .map(|m| match m {
                Message::User { .. } => "user".into(),
                Message::Assistant(_) => "assistant".into(),
                Message::Tool(r) => format!("tool:{}:{}", r.call_id, r.content),
            })
            .collect()
    }

    #[test]
    fn unanswered_calls_get_a_synthetic_result_before_the_next_turn() {
        let out = build(&[
            Message::user_text("go"),
            assistant(Finish::ToolCalls, vec![call("a"), call("b")]),
            result("a"),
            Message::user_text("next"),
        ]);
        let no_result = &model_text::get().no_result;
        assert_eq!(
            roles(&out),
            vec![
                "user".to_owned(),
                "assistant".into(),
                "tool:a:ok".into(),
                format!("tool:b:{no_result}"),
                "user".into(),
            ]
        );
    }

    #[test]
    fn cut_off_turns_keep_only_their_text() {
        let out = build(&[
            Message::user_text("go"),
            assistant(
                Finish::Aborted,
                vec![
                    Part::Reasoning { text: "hm".into() },
                    Part::Text {
                        text: "partial".into(),
                    },
                    call("x"),
                ],
            ),
            result("x"),
            assistant(Finish::Error, vec![]),
        ]);
        assert_eq!(roles(&out), vec!["user", "assistant"]);
        let Message::Assistant(kept) = &out[1] else {
            unreachable!()
        };
        assert_eq!(
            kept.parts,
            vec![Part::Text {
                text: "partial".into()
            }]
        );
    }
}
