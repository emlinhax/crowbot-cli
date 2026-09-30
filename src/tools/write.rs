use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::permissions;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::io::{self, fs::Access, fs::Kind};

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "write",
        include_str!("../../data/tools/write.md"),
        include_str!("../../data/tools/write.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

pub struct Write;

impl Tool for Write {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let target = target::resolve(&cx.app.paths, &args.path).map_err(Refusal::Refused)?;
        if target.kind().map_err(Refusal::Refused)? == Kind::Dir {
            return Err(Refusal::Refused(format!(
                "{} is a directory.",
                target.shown
            )));
        }
        cx.files()
            .check_fresh(&target.path, &target.shown)
            .map_err(Refusal::Refused)?;
        let preview: String = args
            .content
            .lines()
            // All of it: the card shows what fits and counts the rest.
            .map(|l| format!("+{l}\n"))
            .collect();
        Ok(Check::new(target::asks(permissions::EDIT.name, &target)).with_preview(preview))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let target = match target::resolve(&cx.app.paths, &args.path) {
                Ok(target) => target,
                Err(why) => return Output::error(why),
            };
            let _guard = cx.files().lock(&target.path).await;
            if let Err(why) = cx.files().check_fresh(&target.path, &target.shown) {
                return Output::error(why);
            }
            let created = target.kind() == Ok(Kind::Missing);
            if let Err(e) =
                io::fs::write_atomic(&target.path, args.content.as_bytes(), Access::Shared)
            {
                return Output::error(format!("Could not write {}: {e}", target.shown));
            }
            cx.files().saw(&target.path);
            let lines = args.content.lines().count();
            let verb = if created { "Created" } else { "Replaced" };
            Output::ok(format!("{verb} {} ({lines} lines).", target.shown))
                .with_details(json!({"created": created, "lines": lines}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

    #[test]
    fn the_preview_holds_every_new_line_so_the_card_can_count_them() {
        let project = Project::new();
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let check = Write
            .check(
                &json!({"path": "big.txt", "content": content}),
                &project.cx(),
            )
            .unwrap();
        assert_eq!(check.preview.unwrap().lines().count(), 100);
    }

    #[tokio::test]
    async fn creates_new_files_but_only_replaces_read_ones() {
        let project = Project::new();
        let out = Write
            .run(
                json!({"path": "new/a.txt", "content": "hi\n"}),
                &project.cx(),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(project.read("new/a.txt"), "hi\n");

        project.write("old.txt", "keep");
        let refused = Write
            .run(json!({"path": "old.txt", "content": "x"}), &project.cx())
            .await;
        assert!(refused.is_error);
        assert_eq!(project.read("old.txt"), "keep");
    }
}
