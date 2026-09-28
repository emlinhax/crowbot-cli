//! Spots a model repeating the same call, which burns money without progress.

use crate::agent::message::ToolCall;

/// How many times in a row the model has now made `call`, counting this one; `before` is every
/// earlier call, oldest first.
pub fn repeats<'a>(
    before: impl DoubleEndedIterator<Item = &'a ToolCall>,
    call: &ToolCall,
) -> usize {
    1 + before.rev().take_while(|c| same(c, call)).count()
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

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    #[test]
    fn counts_the_unbroken_run_of_identical_calls() {
        let history = [
            call("read", r#"{"path":"a"}"#),
            call("bash", r#"{"command":"ls"}"#),
            call("bash", r#"{ "command": "ls" }"#),
        ];
        assert_eq!(
            repeats(history.iter(), &call("bash", r#"{"command":"ls"}"#)),
            3
        );
        assert_eq!(repeats(history.iter(), &call("read", r#"{"path":"a"}"#)), 1);
    }
}
