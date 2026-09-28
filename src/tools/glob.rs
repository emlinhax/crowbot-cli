use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::io::{self, fs::Kind};
use crate::limits;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "glob",
        include_str!("../../data/tools/glob.md"),
        include_str!("../../data/tools/glob.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

pub struct Glob;

impl Tool for Glob {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let root = target::resolve(&cx.app.paths.project, args.path.as_deref().unwrap_or("."));
        Ok(Check {
            asks: target::asks("search", &root),
        })
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let root = target::resolve(&cx.app.paths.project, args.path.as_deref().unwrap_or("."));
            if io::fs::kind(&root.path) != Kind::Dir {
                return Output::error(format!("{} is not a directory.", root.shown));
            }
            let (dir, pattern) = (root.path.clone(), args.pattern.clone());
            let found = tokio::task::spawn_blocking(move || io::search::glob(&dir, &pattern)).await;
            let mut found = match found {
                Ok(Ok(found)) => found,
                Ok(Err(e)) => return Output::error(format!("Bad pattern: {e}")),
                Err(e) => return Output::error(e.to_string()),
            };
            if found.is_empty() {
                return Output::ok(format!("No files match {}.", args.pattern));
            }
            // Newest first: the file being worked on is usually the one changed last.
            found.sort_by_key(|f| std::cmp::Reverse(f.modified));
            let max = limits::get().tools.max_results.value;
            let mut out: String = found
                .iter()
                .take(max)
                .map(|f| {
                    format!(
                        "{}\n",
                        target::resolve(&cx.app.paths.project, &f.path.to_string_lossy()).shown
                    )
                })
                .collect();
            if found.len() > max {
                let _ = write!(out, "\n[{} more; narrow the pattern.]", found.len() - max);
            }
            Output::ok(out.trim_end().to_owned())
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
        let out = Glob.run(json!({"pattern": "*.rs"}), &project.cx()).await;
        let mut lines: Vec<&str> = out.content.lines().collect();
        lines.sort();
        assert_eq!(lines, vec!["src/a.rs", "src/deep/b.rs"]);
    }
}
