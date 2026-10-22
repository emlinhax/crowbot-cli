use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::many::{self, Many};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::config::limits;
use crate::data;
use crate::io::{self, fs::Kind};
use crate::permission::catalog;
use crate::text::shorten;
use crate::text::template::fill;
use crate::text::truncate;

static SPEC: LazyLock<Spec> = super::spec!("read");

static TEXT: LazyLock<Text> = data::load!("tools/read.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    over_budget: String,
    binary: String,
    empty: String,
    past_end: String,
    more_lines: String,
    list_failed: String,
    empty_dir: String,
    more_entries: String,
}

#[derive(Deserialize)]
pub struct Args {
    path: Many<Item>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    depth: Option<usize>,
}

/// A path, or a path with its own lines, so one call reads different parts of several files (or
/// two parts of one).
#[derive(Deserialize)]
#[serde(untagged)]
enum Item {
    Path(String),
    Part {
        path: String,
        #[serde(default)]
        offset: Option<usize>,
        #[serde(default)]
        limit: Option<usize>,
    },
}

impl Args {
    /// Each path with what to show of it: its own lines, else the call's.
    fn parts(&self) -> Vec<(&str, Page)> {
        let page = Page {
            offset: self.offset,
            limit: self.limit,
            depth: self.depth,
        };
        self.path
            .capped(limits::get().tools.read_batch_max.value)
            .iter()
            .map(|item| match item {
                Item::Path(path) => (path.as_str(), page),
                Item::Part {
                    path,
                    offset,
                    limit,
                } => (
                    path.as_str(),
                    Page {
                        offset: offset.or(page.offset),
                        limit: limit.or(page.limit),
                        ..page
                    },
                ),
            })
            .collect()
    }
}

/// How much of each file or directory to show.
#[derive(Clone, Copy)]
struct Page {
    offset: Option<usize>,
    limit: Option<usize>,
    depth: Option<usize>,
}

pub struct Read;

impl Tool for Read {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let mut asks = Vec::new();
        for (path, _) in args.parts() {
            let target = target::resolve(&cx.app.paths, path)?;
            asks.extend(target::asks(catalog::READ.name, &target));
        }
        Ok(Check::new(asks))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let parts = args.parts();
            if let [(path, page)] = parts[..] {
                return one(cx, path, page);
            }
            let budget = limits::get().tools.read_batch_bytes.value;
            let (mut used, mut read) = (0, 0);
            let mut shown = Vec::new();
            for (path, page) in parts {
                // A soft cap: the file that crosses it stays whole, and the rest are only named.
                let out = if used < budget {
                    one(cx, path, page).unwrap_or_else(Output::error)
                } else {
                    Output::error(fill(&TEXT.over_budget, &[("budget", &budget.to_string())]))
                };
                used += out.content.len();
                read += usize::from(!out.is_error);
                shown.push((format!("{path}:"), out.content));
            }
            let content = many::join(shown);
            if read == 0 {
                return Err(content);
            }
            Ok(Output::ok(content).with_details(json!({ "files": read })))
        })
    }
}

/// One path: a file's numbered lines, or a directory's entries.
fn one(cx: &ToolCx<'_>, path: &str, page: Page) -> Result<Output, String> {
    let target = target::resolve(&cx.app.paths, path)?;
    match target.kind()? {
        Kind::Missing => Err(fill(&target::TEXT.missing, &[("path", &target.shown)])),
        Kind::Dir => list(&target.path, &target.shown, page.depth.unwrap_or(1)),
        Kind::File => {
            let out = file(&target.path, &target.shown, page.offset, page.limit)?;
            cx.files().saw(&target.path);
            Ok(out)
        }
    }
}

fn file(
    path: &std::path::Path,
    shown: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Output, String> {
    let limits = &limits::get().tools;
    let bytes = io::fs::read_bytes(path).map_err(|e| {
        fill(
            &target::TEXT.read_failed,
            &[("path", shown), ("error", &e.to_string())],
        )
    })?;
    let sniff = &bytes[..bytes.len().min(limits.binary_sniff_bytes.value)];
    if sniff.contains(&0) {
        return Err(fill(&TEXT.binary, &[("path", shown)]));
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(Output::ok(fill(&TEXT.empty, &[("path", shown)])));
    }
    let start = offset.unwrap_or(1).max(1);
    if start > lines.len() {
        return Err(fill(
            &TEXT.past_end,
            &[
                ("path", shown),
                ("lines", &lines.len().to_string()),
                ("offset", &start.to_string()),
            ],
        ));
    }
    let want = limit
        .unwrap_or(limits.max_lines.value)
        .min(limits.max_lines.value);
    let numbered: String = lines[start - 1..]
        .iter()
        .take(want)
        .enumerate()
        .map(|(i, line)| {
            let line = shorten::line(line, limits.max_line_chars.value);
            format!("{:>6}\t{line}\n", start + i)
        })
        .collect();
    let cut = truncate::cut(
        &numbered,
        truncate::Keep::Head,
        want,
        limits.max_bytes.value,
    );
    let last = start - 1 + cut.kept_lines.max(1);
    let mut content = cut.text;
    if last < lines.len() {
        content.push_str("\n\n");
        content.push_str(&fill(
            &TEXT.more_lines,
            &[
                ("first", &start.to_string()),
                ("last", &last.to_string()),
                ("total", &lines.len().to_string()),
                ("next", &(last + 1).to_string()),
            ],
        ));
    }
    Ok(Output::ok(content).with_details(json!({"lines": [start, last], "total": lines.len()})))
}

/// One level is everything in the directory; deeper levels leave out what .gitignore ignores.
fn list(path: &std::path::Path, shown: &str, depth: usize) -> Result<Output, String> {
    let limits = &limits::get().tools;
    let max = limits.list_entries.value;
    let entries: Vec<(usize, String, bool)> = if depth <= 1 {
        io::fs::list_dir(path)
            .map_err(|e| {
                fill(
                    &TEXT.list_failed,
                    &[("path", shown), ("error", &e.to_string())],
                )
            })?
            .into_iter()
            .map(|e| (1, e.name, e.is_dir))
            .collect()
    } else {
        io::search::tree(path, depth.min(limits.list_depth_max.value))
            .into_iter()
            .map(|e| (e.depth, e.name, e.is_dir))
            .collect()
    };
    let mut out: String = entries
        .iter()
        .take(max)
        .map(|(depth, name, is_dir)| {
            let indent = "  ".repeat(depth - 1);
            format!("{indent}{name}{}\n", if *is_dir { "/" } else { "" })
        })
        .collect();
    if entries.is_empty() {
        out = fill(&TEXT.empty_dir, &[("path", shown)]);
    } else if entries.len() > max {
        out.push('\n');
        out.push_str(&fill(
            &TEXT.more_entries,
            &[("count", &(entries.len() - max).to_string())],
        ));
    }
    Ok(Output::ok(out.trim_end().to_owned()).with_details(json!({ "entries": entries.len() })))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::tools::testing::Project;

    async fn read(project: &Project, args: Value) -> Output {
        project.run(&Read, args).await
    }

    #[tokio::test]
    async fn numbers_lines_and_pages() {
        let project = Project::new();
        let text: String = (1..=5).map(|n| format!("line {n}\n")).collect();
        project.write("a.txt", &text);
        let out = read(&project, json!({"path": "a.txt", "offset": 2, "limit": 2})).await;
        assert!(!out.is_error);
        assert!(
            out.content.starts_with("     2\tline 2\n     3\tline 3"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Use offset=4 to continue"));
    }

    #[tokio::test]
    async fn lists_directories_and_refuses_binaries() {
        let project = Project::new();
        project.write("d/x.txt", "x");
        project.write("d/sub/y.txt", "y");
        let out = read(&project, json!({"path": "d"})).await;
        assert_eq!(out.content, "sub/\nx.txt");
        crate::io::fs::write_atomic(
            &project.app.paths.project.join("bin"),
            b"\x00\x01",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        assert!(read(&project, json!({"path": "bin"})).await.is_error);
    }

    #[tokio::test]
    async fn reads_several_files_at_once_each_under_its_path() {
        let project = Project::new();
        project.write("a.txt", "alpha\n");
        project.write("b.txt", "beta\n");
        let out = read(&project, json!({"path": ["a.txt", "missing.txt", "b.txt"]})).await;
        assert!(!out.is_error);
        assert!(
            out.content.starts_with("a.txt:\n     1\talpha"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("missing.txt:\nmissing.txt does not exist.")
        );
        assert!(
            out.content.contains("b.txt:\n     1\tbeta"),
            "{}",
            out.content
        );
        assert_eq!(out.details, Some(json!({"files": 2})));
        let path = crate::io::fs::canonical(&project.app.paths.project.join("b.txt"));
        assert!(project.shared.files.check_fresh(&path, "b.txt").is_ok());
    }

    #[tokio::test]
    async fn a_listed_file_can_carry_its_own_lines() {
        let project = Project::new();
        let text: String = (1..=9).map(|n| format!("line {n}\n")).collect();
        project.write("a.txt", &text);
        project.write("b.txt", &text);
        let out = read(
            &project,
            json!({"path": [
                {"path": "a.txt", "offset": 2, "limit": 1},
                {"path": "a.txt", "offset": 8},
                "b.txt",
            ], "limit": 1}),
        )
        .await;
        assert!(!out.is_error);
        for part in [
            "a.txt:\n     2\tline 2\n",
            "a.txt:\n     8\tline 8\n",
            "b.txt:\n     1\tline 1\n",
        ] {
            assert!(out.content.contains(part), "{part:?} in {}", out.content);
        }
        assert!(!out.content.contains("line 9"), "{}", out.content);
        assert_eq!(out.details, Some(json!({"files": 3})));
    }

    #[tokio::test]
    async fn a_deeper_listing_indents_each_level() {
        let project = Project::new();
        project.write("d/x.txt", "x");
        project.write("d/sub/y.txt", "y");
        project.write("d/sub/deep/z.txt", "z");
        let out = read(&project, json!({"path": "d", "depth": 2})).await;
        assert_eq!(out.content, "sub/\n  deep/\n  y.txt\nx.txt");
    }

    #[tokio::test]
    async fn reading_records_the_file_as_seen() {
        let project = Project::new();
        project.write("a.txt", "x");
        // Tools record a file by its resolved path (macOS temp dirs sit behind /var -> /private/var).
        let path = crate::io::fs::canonical(&project.app.paths.project.join("a.txt"));
        assert!(project.shared.files.check_fresh(&path, "a.txt").is_err());
        read(&project, json!({"path": "a.txt"})).await;
        assert!(project.shared.files.check_fresh(&path, "a.txt").is_ok());
    }
}
