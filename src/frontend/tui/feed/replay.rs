//! A conversation read back from its session file, laid out as the blocks it showed live.

use super::{Block, Entry, Feed};
use crate::conversation::message::{Message, Part};

/// How deep a subagent tree is reconstructed on replay, as a guard against a malformed chain of
/// session links; real nesting is a level or two.
const MAX_DEPTH: usize = 8;

impl Feed {
    /// `load_child` opens a subagent's session (by the path its `task` result recorded) so its
    /// calls can be shown under the task, as they were live; it returns nothing for one that is
    /// missing or unreadable, and that task stays flat.
    pub fn replay(&mut self, messages: &[Message], load_child: &dyn Fn(&str) -> Vec<Message>) {
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
                        depth: 0,
                    });
                    self.replay_children(result, 1, load_child);
                }
            }
        }
    }

    /// A subagent's calls, from the session its `task` result links, as indented child cards (its
    /// own prose and thinking stay in its session, only its tool calls show in the tree).
    fn replay_children(
        &mut self,
        task: &crate::conversation::message::ToolResult,
        depth: usize,
        load_child: &dyn Fn(&str) -> Vec<Message>,
    ) {
        if task.name != "task" || depth > MAX_DEPTH {
            return;
        }
        let Some(path) = task.details.as_ref().and_then(|d| d["session"].as_str()) else {
            return;
        };
        let mut arguments: Vec<(String, String)> = Vec::new();
        for message in load_child(path) {
            match message {
                Message::Assistant(reply) => {
                    for part in &reply.parts {
                        if let Part::ToolCall(call) = part {
                            arguments.push((call.id.clone(), call.arguments.clone()));
                        }
                    }
                }
                Message::Tool(result) => {
                    let args = arguments
                        .iter()
                        .find(|(id, _)| *id == result.call_id)
                        .map(|(_, a)| a.clone())
                        .unwrap_or_default();
                    self.entries.push(Entry {
                        block: Block::Tool {
                            arguments: args,
                            result: result.clone(),
                            first: false,
                            depth,
                        },
                        open: false,
                        drawn: None,
                    });
                    self.replay_children(&result, depth + 1, load_child);
                }
                Message::User { .. } => {}
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
        feed.replay(&messages, &|_| Vec::new());
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

    #[test]
    fn a_replayed_task_reconstructs_its_subagents_calls() {
        let parent = [
            Message::Assistant(Assistant::stub(
                Finish::ToolCalls,
                vec![Part::ToolCall(ToolCall {
                    id: "t1".into(),
                    name: "task".into(),
                    arguments: r#"{"description":"dig"}"#.into(),
                })],
            )),
            Message::Tool(ToolResult {
                call_id: "t1".into(),
                name: "task".into(),
                content: "done".into(),
                details: Some(
                    serde_json::json!({"agent": "explore", "calls": 1, "session": "child.jsonl"}),
                ),
                is_error: false,
                asks: Vec::new(),
                refused: None,
                usage: None,
            }),
        ];
        let child = vec![
            Message::Assistant(Assistant::stub(
                Finish::ToolCalls,
                vec![Part::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: r#"{"path":"deep.rs"}"#.into(),
                })],
            )),
            Message::Tool(ToolResult {
                call_id: "c1".into(),
                name: "read".into(),
                content: "x".into(),
                details: None,
                is_error: false,
                asks: Vec::new(),
                refused: None,
                usage: None,
            }),
        ];
        let mut feed = Feed::new();
        feed.replay(&parent, &|path| {
            if path == "child.jsonl" {
                child.clone()
            } else {
                Vec::new()
            }
        });
        // The task at the top level, then its subagent's read indented a level under it.
        let tools: Vec<(String, usize)> = feed
            .entries
            .iter()
            .filter_map(|e| match &e.block {
                Block::Tool { result, depth, .. } => Some((result.name.clone(), *depth)),
                _ => None,
            })
            .collect();
        assert_eq!(tools, [("task".into(), 0), ("read".into(), 1)]);
    }
}
