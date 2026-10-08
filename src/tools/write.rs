use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::io::{self, fs::Access, fs::Kind};
use crate::permission::catalog;
use crate::text::diff;

static SPEC: LazyLock<Spec> = super::spec!("write");

#[derive(Deserialize)]
pub struct Args {
    path: String,
    content: String,
}

pub struct Write;

impl Tool for Write {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let target = target::resolve(&cx.app.paths, &args.path)?;
        if target.kind()? == Kind::Dir {
            return Err(format!("{} is a directory.", target.shown).into());
        }
        cx.files().check_fresh(&target.path, &target.shown)?;
        let preview = diff::added(&args.content);
        Ok(Check::new(target::asks(catalog::EDIT.name, &target)).with_preview(preview))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let target = target::resolve(&cx.app.paths, &args.path)?;
            let _guard = cx.files().lock(&target.path).await;
            cx.files().check_fresh(&target.path, &target.shown)?;
            let created = target.kind() == Ok(Kind::Missing);
            io::fs::write_atomic(&target.path, args.content.as_bytes(), Access::Shared)
                .map_err(|e| format!("Could not write {}: {e}", target.shown))?;
            cx.files().saw(&target.path);
            let lines = args.content.lines().count();
            let verb = if created { "Created" } else { "Replaced" };
            Ok(
                Output::ok(format!("{verb} {} ({lines} lines).", target.shown))
                    .with_details(json!({"created": created, "lines": lines})),
            )
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
        let check = project
            .check(&Write, &json!({"path": "big.txt", "content": content}))
            .unwrap();
        assert_eq!(check.preview.unwrap().lines().count(), 100);
    }

    #[tokio::test]
    async fn creates_new_files_but_only_replaces_read_ones() {
        let project = Project::new();
        let out = project
            .run(&Write, json!({"path": "new/a.txt", "content": "hi\n"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(project.read("new/a.txt"), "hi\n");

        project.write("old.txt", "keep");
        let refused = project
            .run(&Write, json!({"path": "old.txt", "content": "x"}))
            .await;
        assert!(refused.is_error);
        assert_eq!(project.read("old.txt"), "keep");
    }
}
