use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

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
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct Read;

impl Tool for Read {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let target =
            target::resolve(&cx.app.paths.project, &args.path).map_err(Refusal::Refused)?;
        Ok(Check::new(target::asks(permissions::READ.name, &target)))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let target = match target::resolve(&cx.app.paths.project, &args.path) {
                Ok(target) => target,
                Err(why) => return Output::error(why),
            };
            match target.kind() {
                Err(why) => Output::error(why),
                Ok(Kind::Missing) => Output::error(format!("{} does not exist.", target.shown)),
                Ok(Kind::Dir) => list(&target.path, &target.shown),
                Ok(Kind::File) => {
                    let out = file(&target.path, &target.shown, args.offset, args.limit);
                    if !out.is_error {
                        cx.files().saw(&target.path);
                    }
                    out
                }
            }
        })
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

fn list(path: &std::path::Path, shown: &str) -> Output {
    let max = limits::get().tools.list_entries.value;
    let entries = match io::fs::list_dir(path) {
        Ok(entries) => entries,
        Err(e) => return Output::error(format!("Could not list {shown}: {e}")),
    };
    let mut out: String = entries
        .iter()
        .take(max)
        .map(|e| format!("{}{}\n", e.name, if e.is_dir { "/" } else { "" }))
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
    Output::ok(out.trim_end().to_owned())
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
    async fn reading_records_the_file_as_seen() {
        let project = Project::new();
        project.write("a.txt", "x");
        let path = project.app.paths.project.join("a.txt");
        assert!(project.shared.files.check_fresh(&path, "a.txt").is_err());
        read(&project, json!({"path": "a.txt"})).await;
        assert!(project.shared.files.check_fresh(&path, "a.txt").is_ok());
    }
}
