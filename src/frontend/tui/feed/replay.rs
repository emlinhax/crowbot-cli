//! A conversation read back from its session file, laid out as the blocks it showed live.

use super::{Block, Feed};
use crate::conversation::message::{Message, Part};

impl Feed {
    pub fn replay(&mut self, messages: &[Message]) {
        // A tool card shows the call's arguments, which the reply before its result carries.
        let mut arguments = Vec::new();
        // The results of one reply are consecutive; group them as a batch, as the live feed does.
        let mut batch_open = false;
        for message in messages {
            match message {
                Message::User { parts } => {
                    batch_open = false;
                    let text = Part::text_of(parts);
                    if !text.trim().is_empty() {
                        self.user(&text);
                    }
                }
                Message::Assistant(reply) => {
                    batch_open = false;
                    for part in &reply.parts {
                        match part {
                            Part::Reasoning { text } if !text.trim().is_empty() => {
                                self.push(Block::Thinking {
                                    text: text.clone(),
                                    secs: None,
                                });
                            }
                            Part::ToolCall(call) => {
                                arguments.push((call.id.clone(), call.arguments.clone()));
                            }
                            _ => {}
                        }
                    }
                    let text = Part::text_of(&reply.parts);
                    if !text.trim().is_empty() {
                        self.push(Block::Reply {
                            md: text,
                            first: true,
                        });
                    }
                    self.outcome_of(reply);
                }
                Message::Tool(result) => {
                    let arguments = arguments
                        .iter()
                        .find(|(id, _)| *id == result.call_id)
                        .map(|(_, a)| a.clone())
                        .unwrap_or_default();
                    let first = !batch_open;
                    batch_open = true;
                    self.push(Block::Tool {
                        arguments,
                        result: result.clone(),
                        first,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::conversation::message::{Assistant, Finish, Message, Part, ToolCall, ToolResult};
    use crate::frontend::tui::feed::{Block, Feed};

    #[test]
    fn a_conversation_comes_back_as_the_blocks_it_showed() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a.rs"}"#.into(),
        };
        let messages = [
            Message::User {
                parts: vec![
                    Part::Reminder {
                        text: "PLAN mode is on.".into(),
                    },
                    Part::Text {
                        text: "fix it".into(),
                    },
                ],
            },
            Message::Assistant(Assistant::stub(
                Finish::ToolCalls,
                vec![
                    Part::Reasoning {
                        text: "look first".into(),
                    },
                    Part::Text {
                        text: "Reading.".into(),
                    },
                    Part::ToolCall(call),
                ],
            )),
            Message::Tool(ToolResult {
                call_id: "c1".into(),
                name: "read".into(),
                content: "fn main() {}".into(),
                is_error: false,
                details: None,
                asks: Vec::new(),
                refused: None,
                usage: None,
            }),
            Message::Assistant(Assistant::stub(Finish::Length, Vec::new())),
        ];
        let mut feed = Feed::new();
        feed.replay(&messages);
        let kinds: Vec<String> = feed
            .entries
            .iter()
            .map(|e| match &e.block {
                Block::User(text) => format!("user {text}"),
                Block::Thinking { secs, .. } => format!("thinking {secs:?}"),
                Block::Reply { md, .. } => format!("reply {md}"),
                Block::Tool { arguments, .. } => format!("tool {arguments}"),
                Block::Notice { role, .. } => format!("notice {role}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user fix it",
                "thinking None",
                "reply Reading.",
                r#"tool {"path":"a.rs"}"#,
                "notice warn",
            ]
        );
    }
}
