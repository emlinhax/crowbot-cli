use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::target::{self, Target};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::data;
use crate::io::{self, fs::Access, fs::Kind};
use crate::permission::catalog;
use crate::text::diff;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("write");

static TEXT: LazyLock<Text> = data::load!("tools/write.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    created: String,
    replaced: String,
}

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
            return Err(fill(&target::TEXT.is_dir, &[("path", &target.shown)]).into());
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
            let _guard = cx.files().claim(&target.path, &target.shown).await?;
            let created = target.kind() == Ok(Kind::Missing);
            save(cx, &target, &args.content)?;
            let lines = args.content.lines().count();
            let said = if created {
                &TEXT.created
            } else {
                &TEXT.replaced
            };
            Ok(Output::ok(fill(
                said,
                &[("path", &target.shown), ("lines", &lines.to_string())],
            ))
            .with_details(json!({"created": created, "lines": lines})))
        })
    }
}

/// Writes `text` over the target and marks it read, so the model may change it again.
pub fn save(cx: &ToolCx<'_>, target: &Target, text: &str) -> Result<(), String> {
    io::fs::write_atomic(&target.path, text.as_bytes(), Access::Shared).map_err(|e| {
        fill(
            &target::TEXT.write_failed,
            &[("path", &target.shown), ("error", &e.to_string())],
        )
    })?;
    cx.files().saw(&target.path);
    Ok(())
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
