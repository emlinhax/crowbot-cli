use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::permission::catalog;
use std::path::Path;

use crate::config::limits;
use crate::io;
use crate::io::search::{FileHits, Report, SearchError};
use crate::text::shorten;

static SPEC: LazyLock<Spec> = super::spec!("grep");

#[derive(Deserialize)]
pub struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    ignore_case: bool,
    #[serde(default)]
    context: usize,
    #[serde(default)]
    output: Shape,
}

/// What the search answers with.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Shape {
    #[default]
    Content,
    Files,
    Count,
}

pub struct Grep;

impl Tool for Grep {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let root = target::resolve(&cx.app.paths, args.path.as_deref().unwrap_or("."))
            .map_err(Refusal::Refused)?;
        Ok(Check::new(target::asks(catalog::SEARCH.name, &root)))
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let root = match target::search_root(&cx.app.paths, args.path.as_deref()) {
                Ok(root) => root,
                Err(why) => return Output::error(why),
            };
            let base = io::fs::canonical(&cx.app.paths.project);
            let limits = &limits::get().tools;
            let max = limits.max_results.value;
            let report = match args.output {
                Shape::Content => Report::Lines {
                    context: args.context.min(limits.grep_context_max.value),
                },
                Shape::Files => Report::Files,
                Shape::Count => Report::Counts,
            };
            let query = io::search::Grep {
                root: root.path.clone(),
                pattern: args.pattern,
                glob: args.glob,
                ignore_case: args.ignore_case,
                report,
                max,
            };
            let (files, capped) = match io::search::grep(query).await {
                Ok(found) => found,
                Err(SearchError::BadPattern(e)) => {
                    return Output::error(format!("Bad pattern: {e}"));
                }
                Err(e) => return Output::error(e.to_string()),
            };
            if files.is_empty() {
                return Output::ok("No matches.").with_details(json!({"matches": 0}));
            }
            let shown = |path: &Path| target::show(&base, path).0;
            let matched: usize = files.iter().map(|f| f.matched).sum();
            let (mut out, details, unit) = match report {
                Report::Lines { context } => (
                    lines(&files, context, &shown),
                    json!({ "matches": matched }),
                    "matches",
                ),
                Report::Files => (
                    files.iter().map(|f| shown(&f.path) + "\n").collect(),
                    json!({ "files": files.len() }),
                    "files",
                ),
                Report::Counts => (
                    files
                        .iter()
                        .map(|f| format!("{}: {}\n", shown(&f.path), f.matched))
                        .collect(),
                    json!({ "matches": matched, "files": files.len() }),
                    "files",
                ),
            };
            if capped {
                let _ = write!(out, "\n[Stopped at {max} {unit}; narrow the search.]");
            }
            Output::ok(out.trim_end().to_owned()).with_details(details)
        })
    }
}

/// `path:line: text` for each match and `path-line- text` around it, as ripgrep prints them, with
/// `--` between groups that are not next to each other.
fn lines(files: &[FileHits], context: usize, shown: &dyn Fn(&Path) -> String) -> String {
    let cut = limits::get().tools.grep_line_chars.value;
    let mut out = String::new();
    for file in files {
        let name = shown(&file.path);
        let mut last: Option<u64> = None;
        for line in &file.lines {
            let apart = last.is_none_or(|n| line.number > n + 1);
            if context > 0 && apart && !out.is_empty() {
                out.push_str("--\n");
            }
            last = Some(line.number);
            let mark = if line.matched { ':' } else { '-' };
            let text = shorten::line(line.text.trim(), cut);
            let _ = writeln!(out, "{name}{mark}{}{mark} {text}", line.number);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::tools::testing::Project;
    use serde_json::json;

    #[tokio::test]
    async fn reports_path_line_and_text() {
        let project = Project::new();
        project.write("src/a.rs", "fn one() {}\nfn two() {}\n");
        let out = project
            .run(&Grep, json!({"pattern": "fn t\\w+", "glob": "*.rs"}))
            .await;
        assert_eq!(out.content, "src/a.rs:2: fn two() {}");
    }

    #[tokio::test]
    async fn shows_context_lists_files_or_counts_them() {
        let project = Project::new();
        project.write("a.txt", "one\nhit\ntwo\nthree\nfour\nhit\n");
        project.write("b.txt", "hit\nhit\n");
        let grep = |args: Value| async { project.run(&Grep, args).await.content };
        let around = grep(json!({"pattern": "hit", "glob": "a.txt", "context": 1})).await;
        assert_eq!(
            around,
            "a.txt-1- one\na.txt:2: hit\na.txt-3- two\n--\na.txt-5- four\na.txt:6: hit"
        );
        let mut files: Vec<String> = grep(json!({"pattern": "hit", "output": "files"}))
            .await
            .lines()
            .map(str::to_owned)
            .collect();
        files.sort();
        assert_eq!(files, ["a.txt", "b.txt"]);
        let counts = grep(json!({"pattern": "hit", "glob": "b.txt", "output": "count"})).await;
        assert_eq!(counts, "b.txt: 2");
    }
}
