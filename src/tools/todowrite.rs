use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::data;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("todowrite");

static TEXT: LazyLock<Text> = data::load!("tools/todowrite.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    pending: String,
    in_progress: String,
    completed: String,
    open: String,
}

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

    fn run<'a>(
        &'a self,
        args: Self::Args,
        _cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let lines: Vec<String> = args
                .todos
                .iter()
                .map(|t| {
                    let mark = match t.status {
                        Status::Pending => &TEXT.pending,
                        Status::InProgress => &TEXT.in_progress,
                        Status::Completed => &TEXT.completed,
                    };
                    format!("{mark} {}", t.content)
                })
                .collect();
            let open = args
                .todos
                .iter()
                .filter(|t| t.status != Status::Completed)
                .count();
            let open = fill(&TEXT.open, &[("open", &open.to_string())]);
            Ok(Output::ok(format!("{}\n\n{open}", lines.join("\n")))
                .with_details(json!({"todos": args.todos})))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

    #[tokio::test]
    async fn lists_the_todos_back_with_marks_and_the_open_count() {
        let project = Project::new();
        let todos = json!({"todos": [
            {"content": "a", "status": "completed"},
            {"content": "b", "status": "in_progress"},
            {"content": "c", "status": "pending"},
        ]});
        let out = project.run(&TodoWrite, todos).await;
        assert_eq!(out.content, "[x] a\n[>] b\n[ ] c\n\n2 open.");
    }
}
