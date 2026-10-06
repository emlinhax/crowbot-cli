use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::many::{self, Many};
use super::permissions;
use super::truncate;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::io::{self, fs::Kind};
use crate::limits;
use crate::text::shorten;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "read",
        include_str!("../../data/tools/read.md"),
        include_str!("../../data/tools/read.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    path: Many<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    depth: Option<usize>,
}

impl Args {
    fn paths(self) -> (Vec<String>, Page) {
        let page = Page {
            offset: self.offset,
            limit: self.limit,
            depth: self.depth,
        };
        (
            self.path.capped(limits::get().tools.read_batch_max.value),
            page,
        )
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
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let (paths, _) = parse::<Args>(args)?.paths();
        let mut asks = Vec::new();
        for path in &paths {
            let target = target::resolve(&cx.app.paths, path).map_err(Refusal::Refused)?;
            asks.extend(target::asks(permissions::READ.name, &target));
        }
        Ok(Check::new(asks))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let (paths, page) = match parse_or_fail::<Args>(&args) {
                Ok(args) => args.paths(),
                Err(out) => return out,
            };
            if let [path] = paths.as_slice() {
                return one(cx, path, page);
            }
            let budget = limits::get().tools.read_batch_bytes.value;
            let (mut used, mut read) = (0, 0);
            let mut parts = Vec::new();
            for path in &paths {
                // A soft cap: the file that crosses it stays whole, and the rest are only named.
                let out = if used < budget {
                    one(cx, path, page)
                } else {
                    Output::error(format!(
                        "Not read: this batch already holds {budget} bytes. Read it on its own."
                    ))
                };
                used += out.content.len();
                read += usize::from(!out.is_error);
                parts.push((format!("{path}:"), out.content));
            }
            let content = many::join(parts);
            if read == 0 {
                return Output::error(content);
            }
            Output::ok(content).with_details(json!({ "files": read }))
        })
    }
}

/// One path: a file's numbered lines, or a directory's entries.
fn one(cx: &ToolCx<'_>, path: &str, page: Page) -> Output {
    let target = match target::resolve(&cx.app.paths, path) {
        Ok(target) => target,
        Err(why) => return Output::error(why),
    };
    match target.kind() {
        Err(why) => Output::error(why),
        Ok(Kind::Missing) => Output::error(format!("{} does not exist.", target.shown)),
        Ok(Kind::Dir) => list(&target.path, &target.shown, page.depth.unwrap_or(1)),
        Ok(Kind::File) => {
            let out = file(&target.path, &target.shown, page.offset, page.limit);
            if !out.is_error {
                cx.files().saw(&target.path);
            }
            out
        }
    }
}

fn file(
    path: &std::path::Path,
    shown: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Output {
    let limits = &limits::get().tools;
    let bytes = match io::fs::read_bytes(path) {
        Ok(bytes) => bytes,
        Err(e) => return Output::error(format!("Could not read {shown}: {e}")),
    };
    let sniff = &bytes[..bytes.len().min(limits.binary_sniff_bytes.value)];
    if sniff.contains(&0) {
        return Output::error(format!(
            "{shown} is a binary file; it cannot be shown as text."
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Output::ok(format!("{shown} is empty."));
    }
    let start = offset.unwrap_or(1).max(1);
    if start > lines.len() {
        return Output::error(format!(
            "{shown} has {} lines; offset {start} is past the end.",
            lines.len()
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
        let _ = write!(
            content,
            "\n\n[Showing lines {start}-{last} of {}. Use offset={} to continue.]",
            lines.len(),
            last + 1
        );
    }
    Output::ok(content).with_details(json!({"lines": [start, last], "total": lines.len()}))
}

/// One level is everything in the directory; deeper levels leave out what .gitignore ignores.
fn list(path: &std::path::Path, shown: &str, depth: usize) -> Output {
    let limits = &limits::get().tools;
    let max = limits.list_entries.value;
    let entries: Vec<(usize, String, bool)> = if depth <= 1 {
        match io::fs::list_dir(path) {
            Ok(entries) => entries.into_iter().map(|e| (1, e.name, e.is_dir)).collect(),
            Err(e) => return Output::error(format!("Could not list {shown}: {e}")),
        }
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
        out = format!("{shown} is an empty directory.");
    } else if entries.len() > max {
        let _ = write!(
            out,
            "\n[{} more entries; use glob to search.]",
            entries.len() - max
        );
    }
    Output::ok(out.trim_end().to_owned()).with_details(json!({ "entries": entries.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

    async fn read(project: &Project, args: Value) -> Output {
        Read.run(args, &project.cx()).await
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
