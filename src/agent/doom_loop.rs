//! Spots a model repeating the same call, which burns money without progress.

use crate::agent::message::{Message, ToolCall};

/// How many times in a row the model has now made `reply[i]`, counting this one. The window is
/// the current user turn: a call the user asks for again in a new message starts over at 1.
/// CEILING: interleaved repeats ([a, b, a, b]) never count; upgrade: count by (name, args)
/// within the window.
pub fn repeats(history: &[Message], reply: &[ToolCall], i: usize) -> usize {
    let turn = history
        .iter()
        .rposition(|m| matches!(m, Message::User { .. }))
        .map_or(0, |u| u + 1);
    let mut before: Vec<&ToolCall> = history[turn..]
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.tool_calls()),
            _ => None,
        })
        .flatten()
        .collect();
    before.extend(&reply[..i]);
    1 + before
        .iter()
        .rev()
        .take_while(|c| same(c, &reply[i]))
        .count()
}

/// Same tool and same arguments, however the JSON was spaced.
fn same(a: &ToolCall, b: &ToolCall) -> bool {
    if a.name != b.name {
        return false;
    }
    match (
        serde_json::from_str::<serde_json::Value>(&a.arguments),
        serde_json::from_str::<serde_json::Value>(&b.arguments),
    ) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.arguments == b.arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::{Assistant, Finish, Part};

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn made(calls: Vec<ToolCall>) -> Message {
        Message::Assistant(Assistant {
            parts: calls.into_iter().map(Part::ToolCall).collect(),
            model: "m".into(),
            effort: None,
            usage: None,
            finish: Finish::ToolCalls,
            error: None,
            request_id: None,
        })
    }

    #[test]
    fn counts_the_unbroken_run_of_identical_calls() {
        let history = [
            Message::user_text("go"),
            made(vec![call("read", r#"{"path":"a"}"#)]),
            made(vec![call("bash", r#"{"command":"ls"}"#)]),
        ];
        let reply = [
            call("bash", r#"{ "command": "ls" }"#),
            call("bash", r#"{"command":"ls"}"#),
            call("read", r#"{"path":"a"}"#),
        ];
        assert_eq!(repeats(&history, &reply, 1), 3);
        assert_eq!(repeats(&history, &reply, 2), 1);
    }

    #[test]
    fn a_new_user_message_starts_the_count_over() {
        let ls = || call("bash", r#"{"command":"ls"}"#);
        let history = [
            Message::user_text("list it"),
            made(vec![ls()]),
            made(vec![ls()]),
            Message::user_text("list it again"),
        ];
        assert_eq!(repeats(&history, &[ls()], 0), 1);
    }
}
