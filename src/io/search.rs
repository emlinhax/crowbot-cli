//! File search, embedded from ripgrep's crates so nothing needs downloading. Honors .gitignore.
//! Blocking: callers run it off the async threads.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobMatcher};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;

pub struct Found {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

pub struct Hit {
    pub path: PathBuf,
    pub line: u64,
    pub text: String,
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

pub struct Grep<'a> {
    pub root: &'a Path,
    pub pattern: &'a str,
    pub glob: Option<&'a str>,
    pub ignore_case: bool,
    /// Stop after this many hits.
    pub max: usize,
}

/// Matching lines, and whether the search stopped at `max`.
pub fn grep(query: &Grep<'_>) -> Result<(Vec<Hit>, bool), String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.ignore_case)
        .build(query.pattern)
        .map_err(|e| e.to_string())?;
    let filter = query.glob.map(compile).transpose()?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build();
    let mut hits = Vec::new();
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
        let _ = searcher.search_path(
            &matcher,
            path,
            UTF8(|line, text| {
                hits.push(Hit {
                    path: path.to_path_buf(),
                    line,
                    text: text.trim_end_matches(['\n', '\r']).to_owned(),
                });
                Ok(hits.len() < query.max)
            }),
        );
        if hits.len() >= query.max {
            return Ok((hits, true));
        }
    }
    Ok((hits, false))
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
    fn grep_finds_lines_respects_filters_and_caps() {
        let dir = tree();
        let query = Grep {
            root: dir.path(),
            pattern: r"\brun\b",
            glob: Some("*.rs"),
            ignore_case: false,
            max: 10,
        };
        let (hits, truncated) = grep(&query).unwrap();
        assert!(!truncated);
        let mut lines: Vec<String> = hits
            .iter()
            .map(|h| format!("{}:{}", relative_slashes(dir.path(), &h.path), h.line))
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
}
