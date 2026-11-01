use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::many::Many;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::data;
use crate::permission::catalog;
use std::path::Path;

use crate::config::limits;
use crate::io;
use crate::io::search::{FileHits, Report};
use crate::text::shorten;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("grep");

static TEXT: LazyLock<Text> = data::load!("tools/grep.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    none: String,
    stopped_matches: String,
    stopped_files: String,
}

#[derive(Deserialize)]
pub struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<Many<String>>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    ignore_case: bool,
    #[serde(default)]
    context: usize,
    #[serde(default)]
    output: Shape,
}

impl Args {
    /// Where to search: the listed files and directories, else the working directory.
    fn paths(&self) -> Vec<&str> {
        match self.path.as_ref().map(Many::all) {
            Some(paths) if !paths.is_empty() => paths.iter().map(String::as_str).collect(),
            _ => vec!["."],
        }
    }
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
        let mut asks = Vec::new();
        for path in args.paths() {
            let root = target::resolve(&cx.app.paths, path)?;
            asks.extend(target::asks(catalog::SEARCH.name, &root));
        }
        Ok(Check::new(asks))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let roots = args
                .paths()
                .into_iter()
                .map(|path| target::search_root(&cx.app.paths, Some(path)).map(|t| t.path))
                .collect::<Result<Vec<_>, _>>()?;
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
                roots,
                pattern: args.pattern,
                glob: args.glob,
                ignore_case: args.ignore_case,
                report,
                max,
            };
            let (files, capped) = io::search::grep(query).await.map_err(|e| e.to_string())?;
            if files.is_empty() {
                return Ok(Output::ok(TEXT.none.clone()).with_details(json!({"matches": 0})));
            }
            let shown = |path: &Path| target::show(&base, path).0;
            let matched: usize = files.iter().map(|f| f.matched).sum();
            let (mut out, details, stopped) = match report {
                Report::Lines { context } => (
                    lines(&files, context, &shown),
                    json!({ "matches": matched }),
                    &TEXT.stopped_matches,
                ),
                Report::Files => (
                    files.iter().map(|f| shown(&f.path) + "\n").collect(),
                    json!({ "files": files.len() }),
                    &TEXT.stopped_files,
                ),
                Report::Counts => (
                    files
                        .iter()
                        .map(|f| format!("{}: {}\n", shown(&f.path), f.matched))
                        .collect(),
                    json!({ "matches": matched, "files": files.len() }),
                    &TEXT.stopped_files,
                ),
            };
            if capped {
                out.push('\n');
                out.push_str(&fill(stopped, &[("max", &max.to_string())]));
            }
            Ok(Output::ok(out.trim_end().to_owned()).with_details(details))
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

    #[tokio::test]
    async fn searches_a_list_of_paths_each_file_once() {
        let project = Project::new();
        project.write("a.txt", "hit\n");
        project.write("d/b.txt", "hit\n");
        project.write("c.txt", "hit\n");
        let out = project
            .run(
                &Grep,
                json!({"pattern": "hit", "path": ["a.txt", "d", "d/b.txt"], "output": "files"}),
            )
            .await;
        assert_eq!(out.content, "a.txt\nd/b.txt");
        let missing = project
            .run(&Grep, json!({"pattern": "hit", "path": ["a.txt", "nope"]}))
            .await;
        assert!(missing.is_error, "{}", missing.content);
    }
}
