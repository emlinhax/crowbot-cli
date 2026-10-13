//! `make battery-setup`: clone each outside repo into the cache at its commit and set it up, then
//! prove the corpus fair: every repo's suite passes untouched, every planted bug fails its check.

use std::time::Duration;

use anyhow::{Context, bail};

use crate::cmd::{self, git};
use crate::corpus::{Repo, Settings, Task};
use crate::project;
use crate::run;

/// A cold dependency install or fetch; minutes, not hours.
const SETUP_LIMIT: Duration = Duration::from_secs(1800);

pub async fn cache(repo: &Repo) -> anyhow::Result<()> {
    let (Some(url), Some(commit)) = (&repo.url, &repo.commit) else {
        return Ok(());
    };
    let cache = repo.cache();
    if !cache.join(".git").exists() {
        let parent = cache.parent().context("cache dir")?;
        std::fs::create_dir_all(parent)?;
        git(&["clone", "-q", url, &cache.to_string_lossy()], parent).await?;
    }
    if git(&["rev-parse", "HEAD"], &cache).await?.trim() != commit {
        git(&["fetch", "-q", "origin", commit], &cache).await?;
        git(&["checkout", "-q", "--detach", commit], &cache).await?;
    }
    if let Some(line) = &repo.setup {
        let ran = cmd::sh(&repo.fill(line), &cache, &repo.env(), SETUP_LIMIT).await;
        if !ran.ok {
            bail!("{}: setup failed:\n{}", repo.id, ran.output.trim());
        }
    }
    Ok(())
}

/// What is unfair about the corpus; empty when nothing is.
pub async fn verify(repos: &[Repo], tasks: &[Task], settings: &Settings) -> Vec<String> {
    let mut problems = Vec::new();
    for repo in repos {
        match passes(repo, None, &repo.test, settings).await {
            Ok(true) => eprintln!("battery: {} passes untouched", repo.id),
            Ok(false) => problems.push(format!("{}: `{}` fails untouched", repo.id, repo.test)),
            Err(e) => problems.push(format!("{}: {e:#}", repo.id)),
        }
    }
    for task in tasks.iter().filter(|t| t.patch.is_some()) {
        let repo = repos.iter().find(|r| r.id == task.repo).expect("checked");
        match passes(repo, task.patch.as_deref(), &task.check, settings).await {
            Ok(false) => eprintln!("battery: {} fails before the fix", task.id),
            Ok(true) => problems.push(format!("{}: its check passes before any fix", task.id)),
            Err(e) => problems.push(format!("{}: {e:#}", task.id)),
        }
    }
    problems
}

async fn passes(
    repo: &Repo,
    patch: Option<&str>,
    check: &str,
    settings: &Settings,
) -> anyhow::Result<bool> {
    let parent = tempfile::tempdir()?;
    let copy = parent.path().join(&repo.id);
    project::make(repo, patch, &copy).await?;
    let reply = parent.path().join("reply.txt");
    std::fs::write(&reply, "")?;
    let line = run::check_line(check, &reply, &parent.path().join("no-plan.md"));
    let ran = cmd::sh(&line, &copy, &repo.env(), settings.check_secs.secs()).await;
    if !ran.ok && patch.is_none() {
        eprintln!("{}", ran.output);
    }
    Ok(ran.ok)
}
