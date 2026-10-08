use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::permission::catalog;
use crate::permission::gate::Ask;

static SPEC: LazyLock<Spec> = super::spec!("todowrite");

#[derive(Deserialize)]
pub struct Args {
    todos: Vec<Todo>,
}

#[derive(Serialize, Deserialize)]
struct Todo {
    content: String,
    status: Status,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Pending,
    InProgress,
    Completed,
}

/// The latest call's list is the current list, so the transcript itself is the state.
pub struct TodoWrite;

impl Tool for TodoWrite {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, _args: &Self::Args, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(Check::new(vec![Ask::new(catalog::TODO.name, "*")]))
    }

    fn run<'a>(&'a self, args: Self::Args, _cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let lines: Vec<String> = args
                .todos
                .iter()
                .map(|t| {
                    let mark = match t.status {
                        Status::Pending => "[ ]",
                        Status::InProgress => "[>]",
                        Status::Completed => "[x]",
                    };
                    format!("{mark} {}", t.content)
                })
                .collect();
            let open = args
                .todos
                .iter()
                .filter(|t| t.status != Status::Completed)
                .count();
            Output::ok(format!("{}\n\n{open} open.", lines.join("\n")))
                .with_details(json!({"todos": args.todos}))
        })
    }
}
