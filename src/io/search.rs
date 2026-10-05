//! File search, embedded from ripgrep's crates so nothing needs downloading. Honors .gitignore.
//! Blocking: callers run it off the async threads.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobMatcher};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;

pub struct Found {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

/// What a content search reports about each file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// The matching lines, with this many lines of context before and after each.
    Lines { context: usize },
    /// Only that the file matched; each file stops at its first match.
    Files,
    /// How many lines matched.
    Counts,
}

/// One file's part of a search: its lines (matches and their context) under `Report::Lines`, and
/// how many lines matched.
pub struct FileHits {
    pub path: PathBuf,
    pub lines: Vec<HitLine>,
    pub matched: usize,
}

pub struct HitLine {
    pub number: u64,
    pub text: String,
    /// False for a line shown as context.
    pub matched: bool,
}

/// Files under `root` matching `pattern`. A pattern without `/` matches file names at any depth.
pub fn glob(root: &Path, pattern: &str) -> Result<Vec<Found>, String> {
    let matcher = compile(pattern)?;
    let by_name = !pattern.contains('/');
    let mut found = Vec::new();
    for entry in WalkBuilder::new(root).build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = relative_slashes(root, path);
        let hit = matcher.is_match(&relative)
            || (by_name
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| matcher.is_match(n)));
        if hit {
            found.push(Found {
                path: path.to_path_buf(),
                modified: entry.metadata().ok().and_then(|m| m.modified().ok()),
            });
        }
    }
    Ok(found)
}

pub struct TreeEntry {
    pub name: String,
    /// 1 for an entry of `root` itself.
    pub depth: usize,
    pub is_dir: bool,
}

/// What lies under `root`, `depth` levels down, in tree order with each directory's entries by
/// name, less what .gitignore ignores and `.git` itself.
pub fn tree(root: &Path, depth: usize) -> Vec<TreeEntry> {
    WalkBuilder::new(root)
        .max_depth(Some(depth))
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .map(|e| TreeEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            depth: e.depth(),
            is_dir: e.file_type().is_some_and(|t| t.is_dir()),
        })
        .collect()
}

pub struct Grep<'a> {
    pub root: &'a Path,
    pub pattern: &'a str,
    pub glob: Option<&'a str>,
    pub ignore_case: bool,
    pub report: Report,
    /// Stop after this many matching lines (`Lines`) or files (`Files`, `Counts`).
    pub max: usize,
}

/// The files that matched, in walk order, and whether the search stopped at `max`.
pub fn grep(query: &Grep<'_>) -> Result<(Vec<FileHits>, bool), String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.ignore_case)
        .build(query.pattern)
        .map_err(|e| e.to_string())?;
    let filter = query.glob.map(compile).transpose()?;
    let context = match query.report {
        Report::Lines { context } => context,
        Report::Files | Report::Counts => 0,
    };
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .before_context(context)
        .after_context(context)
        .build();
    let mut files: Vec<FileHits> = Vec::new();
    let mut lines = 0;
    for entry in WalkBuilder::new(query.root).build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(filter) = &filter {
            let relative = relative_slashes(query.root, path);
            let name = entry.file_name().to_string_lossy();
            if !filter.is_match(&relative) && !filter.is_match(name.as_ref()) {
                continue;
            }
        }
        let mut sink = Collect {
            report: query.report,
            lines: Vec::new(),
            matched: 0,
            room: query.max.saturating_sub(lines),
        };
        let _ = searcher.search_path(&matcher, path, &mut sink);
        if sink.matched == 0 {
            continue;
        }
        lines += sink.matched;
        files.push(FileHits {
            path: path.to_path_buf(),
            lines: sink.lines,
            matched: sink.matched,
        });
        let reached = match query.report {
            Report::Lines { .. } => lines,
            Report::Files | Report::Counts => files.len(),
        };
        if reached >= query.max {
            return Ok((files, true));
        }
    }
    Ok((files, false))
}

/// Gathers one file's matches for `grep`.
struct Collect {
    report: Report,
    lines: Vec<HitLine>,
    matched: usize,
    /// Matching lines the search may still take before its cap.
    room: usize,
}

impl Collect {
    fn push(&mut self, number: Option<u64>, bytes: &[u8], matched: bool) {
        self.lines.push(HitLine {
            number: number.unwrap_or(0),
            text: String::from_utf8_lossy(bytes)
                .trim_end_matches(['\n', '\r'])
                .to_owned(),
            matched,
        });
    }
}

impl Sink for Collect {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.matched += 1;
        match self.report {
            Report::Files => Ok(false),
            Report::Counts => Ok(true),
            Report::Lines { .. } => {
                self.push(m.line_number(), m.bytes(), true);
                Ok(self.matched < self.room)
            }
        }
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        self.push(c.line_number(), c.bytes(), false);
        Ok(true)
    }
}

fn compile(pattern: &str) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| e.to_string())
}

fn relative_slashes(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            ("src/main.rs", "fn main() {\n    run();\n}\n"),
            ("src/lib/util.rs", "pub fn run() {}\n"),
            ("notes.md", "run the thing\n"),
            ("target/junk.rs", "fn run() {}\n"),
            (".gitignore", "target/\n"),
        ] {
            crate::io::fs::write_atomic(
                &dir.path().join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }
        // `ignore` only applies .gitignore inside a git repository.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        dir
    }

    fn names(found: &[PathBuf], root: &Path) -> Vec<String> {
        let mut names: Vec<String> = found.iter().map(|p| relative_slashes(root, p)).collect();
        names.sort();
        names
    }

    #[test]
    fn glob_matches_names_anywhere_and_paths_exactly() {
        let dir = tree();
        let all: Vec<PathBuf> = glob(dir.path(), "*.rs")
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(
            names(&all, dir.path()),
            vec!["src/lib/util.rs", "src/main.rs"]
        );
        let top: Vec<PathBuf> = glob(dir.path(), "src/*.rs")
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(names(&top, dir.path()), vec!["src/main.rs"]);
    }

    #[test]
    fn tree_goes_down_in_order_and_skips_what_git_ignores() {
        let dir = tree();
        let shown: Vec<String> = super::tree(dir.path(), 2)
            .iter()
            .map(|e| {
                format!(
                    "{}{}{}",
                    "  ".repeat(e.depth - 1),
                    e.name,
                    if e.is_dir { "/" } else { "" }
                )
            })
            .collect();
        assert_eq!(
            shown,
            [".gitignore", "notes.md", "src/", "  lib/", "  main.rs"]
        );
    }

    #[test]
    fn grep_finds_lines_respects_filters_and_caps() {
        let dir = tree();
        let query = Grep {
            root: dir.path(),
            pattern: r"\brun\b",
            glob: Some("*.rs"),
            ignore_case: false,
            report: Report::Lines { context: 0 },
            max: 10,
        };
        let (hits, truncated) = grep(&query).unwrap();
        assert!(!truncated);
        let mut lines: Vec<String> = hits
            .iter()
            .flat_map(|f| {
                let name = relative_slashes(dir.path(), &f.path);
                f.lines.iter().map(move |l| format!("{name}:{}", l.number))
            })
            .collect();
        lines.sort();
        assert_eq!(lines, vec!["src/lib/util.rs:1", "src/main.rs:2"]);
        let (capped, truncated) = grep(&Grep {
            max: 1,
            glob: None,
            ..query
        })
        .unwrap();
        assert_eq!(capped.len(), 1);
        assert!(truncated);
    }

    #[test]
    fn grep_reports_context_files_or_counts() {
        let dir = tree();
        let query = |report| Grep {
            root: dir.path(),
            pattern: "run",
            glob: Some("main.rs"),
            ignore_case: false,
            report,
            max: 10,
        };
        let (hits, _) = grep(&query(Report::Lines { context: 1 })).unwrap();
        let shown: Vec<(u64, bool)> = hits[0]
            .lines
            .iter()
            .map(|l| (l.number, l.matched))
            .collect();
        assert_eq!(shown, [(1, false), (2, true), (3, false)]);
        let (hits, _) = grep(&query(Report::Files)).unwrap();
        assert!(hits[0].lines.is_empty() && hits[0].matched == 1);
        let (hits, _) = grep(&Grep {
            glob: None,
            ..query(Report::Counts)
        })
        .unwrap();
        let total: usize = hits.iter().map(|f| f.matched).sum();
        assert_eq!((hits.len(), total), (3, 3));
    }
}
