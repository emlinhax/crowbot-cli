//! The opening orientation handed to the model on its first turn: where it is, what is in the
//! working directory, and the repo's state — so it starts from facts instead of spending turns
//! discovering them, or guessing. Built once per conversation and injected as a reminder on the
//! first prompt (run.rs), so it never touches the cached system prompt.

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use crate::config::paths::Paths;
use crate::data;
use crate::io;
use crate::text::template::fill;

const FRAME: &str = data::raw!("prompts/orient.md");

// Enough to show what kind of project this is without flooding the first turn.
// CEILING: fixed caps; a very large tree still only shows its top level. Upgrade: a shallow tree.
const MAX_ENTRIES: usize = 50;
const MAX_STATUS: usize = 15;
const MAX_COMMITS: usize = 5;

/// A short environment snapshot for the first prompt: the working dir, its top-level entries, and
/// the git state when it is a work tree.
pub async fn snapshot(paths: &Paths) -> String {
    let dir = &paths.project;
    let files = listing(dir);
    let git = git_snapshot(dir).await.unwrap_or_default();
    fill(
        FRAME,
        &[
            ("cwd", &dir.display().to_string()),
            ("files", &files),
            ("git", &git),
        ],
    )
    .trim_end()
    .to_owned()
}

/// The top-level entries, directories marked with a trailing slash, capped.
fn listing(dir: &Path) -> String {
    let Ok(entries) = io::fs::list_dir(dir) else {
        return String::new();
    };
    if entries.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = entries
        .iter()
        .take(MAX_ENTRIES)
        .map(|e| match e.is_dir {
            true => format!("{}/", e.name),
            false => e.name.clone(),
        })
        .collect();
    let more = entries.len().saturating_sub(shown.len());
    let tail = match more {
        0 => String::new(),
        n => format!("  … (+{n} more)"),
    };
    format!("Files here: {}{tail}\n", shown.join("  "))
}

/// Branch, short status and a few recent commits, or `None` when git is missing or this is no
/// work tree.
async fn git_snapshot(dir: &Path) -> Option<String> {
    let git = io::proc::which("git")?;
    let branch = git_run(&git, dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let mut out = String::new();
    let _ = writeln!(out, "Git branch: {branch}");
    match git_run(&git, dir, &["status", "--short"])
        .await
        .filter(|s| !s.is_empty())
    {
        Some(status) => {
            let lines: Vec<&str> = status.lines().take(MAX_STATUS).collect();
            let more = status.lines().count().saturating_sub(lines.len());
            let tail = match more {
                0 => String::new(),
                n => format!("\n  … (+{n} more)"),
            };
            let _ = writeln!(out, "Uncommitted changes:\n  {}{tail}", lines.join("\n  "));
        }
        None => {
            let _ = writeln!(out, "Working tree clean.");
        }
    }
    let n = MAX_COMMITS.to_string();
    if let Some(log) = git_run(&git, dir, &["log", "--oneline", "-n", &n])
        .await
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(out, "Recent commits:\n  {}", log.replace('\n', "\n  "));
    }
    Some(out)
}

async fn git_run(git: &Path, dir: &Path, args: &[&str]) -> Option<String> {
    let argv: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let captured = io::proc::exec(&io::proc::Program {
        program: git,
        args: &argv,
        cwd: dir,
        env: &[],
        unset: &[],
        timeout: Duration::from_secs(5),
    })
    .await
    .ok()?;
    captured
        .ok()
        .then(|| String::from_utf8_lossy(&captured.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_snapshot_names_the_directory_and_its_files() {
        let root = tempfile::tempdir().unwrap();
        crate::io::fs::write_atomic(
            &root.path().join("hello.txt"),
            b"hi",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().to_path_buf());
        let snap = snapshot(&paths).await;
        assert!(snap.contains("<workspace>"), "{snap}");
        assert!(snap.contains("hello.txt"), "{snap}");
        assert!(snap.contains(&root.path().display().to_string()), "{snap}");
    }
}
