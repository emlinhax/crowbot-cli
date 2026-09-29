use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::io::{self, fs::Kind};
use crate::limits;
use crate::text::shorten;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "grep",
        include_str!("../../data/tools/grep.md"),
        include_str!("../../data/tools/grep.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    ignore_case: bool,
}

pub struct Grep;

impl Tool for Grep {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let root = target::resolve(&cx.app.paths.project, args.path.as_deref().unwrap_or("."));
        Ok(Check::new(target::asks("search", &root)))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let root = target::resolve(&cx.app.paths.project, args.path.as_deref().unwrap_or("."));
            if io::fs::kind(&root.path) == Kind::Missing {
                return Output::error(format!("{} does not exist.", root.shown));
            }
            let limits = &limits::get().tools;
            let max = limits.max_results.value;
            let dir = root.path.clone();
            let searched = tokio::task::spawn_blocking(move || {
                io::search::grep(&io::search::Grep {
                    root: &dir,
                    pattern: &args.pattern,
                    glob: args.glob.as_deref(),
                    ignore_case: args.ignore_case,
                    max,
                })
            })
            .await;
            let (hits, capped) = match searched {
                Ok(Ok(found)) => found,
                Ok(Err(e)) => return Output::error(format!("Bad pattern: {e}")),
                Err(e) => return Output::error(e.to_string()),
            };
            if hits.is_empty() {
                return Output::ok("No matches.");
            }
            let mut out: String = hits
                .iter()
                .map(|h| {
                    let shown =
                        target::resolve(&cx.app.paths.project, &h.path.to_string_lossy()).shown;
                    format!(
                        "{shown}:{}: {}\n",
                        h.line,
                        shorten::line(h.text.trim(), limits.grep_line_chars.value)
                    )
                })
                .collect();
            if capped {
                let _ = write!(out, "\n[Stopped at {max} matches; narrow the search.]");
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
    async fn reports_path_line_and_text() {
        let project = Project::new();
        project.write("src/a.rs", "fn one() {}\nfn two() {}\n");
        let out = Grep
            .run(
                json!({"pattern": "fn t\\w+", "glob": "*.rs"}),
                &project.cx(),
            )
            .await;
        assert_eq!(out.content, "src/a.rs:2: fn two() {}");
    }
}
