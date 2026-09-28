use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::permission::gate::Ask;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "todowrite",
        include_str!("../../data/tools/todowrite.md"),
        include_str!("../../data/tools/todowrite.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
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
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let _: Args = parse(args)?;
        Ok(Check {
            asks: vec![Ask::new("todo", "*")],
        })
    }

    fn run<'a>(&'a self, args: Value, _cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
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
