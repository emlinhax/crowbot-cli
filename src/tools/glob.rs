use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::config::limits;
use crate::io::{self, fs::Kind};
use crate::permission::catalog;

static SPEC: LazyLock<Spec> = super::spec!("glob");

#[derive(Deserialize)]
pub struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

pub struct Glob;

impl Tool for Glob {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let root = target::resolve(&cx.app.paths, args.path.as_deref().unwrap_or("."))?;
        Ok(Check::new(target::asks(catalog::SEARCH.name, &root)))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let root = target::search_root(&cx.app.paths, args.path.as_deref())?;
            if root.kind()? != Kind::Dir {
                return Err(format!("{} is not a directory.", root.shown));
            }
            let base = io::fs::canonical(&cx.app.paths.project);
            let mut found = io::search::glob(root.path.clone(), args.pattern.clone())
                .await
                .map_err(|e| e.to_string())?;
            if found.is_empty() {
                return Ok(Output::ok(format!("No files match {}.", args.pattern))
                    .with_details(json!({"files": 0})));
            }
            // Newest first: the file being worked on is usually the one changed last.
            found.sort_by_key(|f| std::cmp::Reverse(f.modified));
            let max = limits::get().tools.max_results.value;
            let mut out: String = found
                .iter()
                .take(max)
                .map(|f| format!("{}\n", target::show(&base, &f.path).0))
                .collect();
            if found.len() > max {
                let _ = write!(out, "\n[{} more; narrow the pattern.]", found.len() - max);
            }
            Ok(Output::ok(out.trim_end().to_owned()).with_details(json!({"files": found.len()})))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;
    use serde_json::json;

    #[tokio::test]
    async fn lists_matches_relative_to_the_project() {
        let project = Project::new();
        project.write("src/a.rs", "");
        project.write("src/deep/b.rs", "");
        project.write("c.txt", "");
        let out = project.run(&Glob, json!({"pattern": "*.rs"})).await;
        let mut lines: Vec<&str> = out.content.lines().collect();
        lines.sort();
        assert_eq!(lines, vec!["src/a.rs", "src/deep/b.rs"]);
    }
}
