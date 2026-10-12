//! A run's own copy of its repo: the pinned files, the task's patch, one commit, then the repo's
//! `prepare`. History is left behind, so `git log` cannot give a planted bug away.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::cmd::{self, git};
use crate::corpus::{Repo, bench};

/// How long a repo's `prepare` may take; it links what setup installed, so seconds.
const PREPARE_LIMIT: Duration = Duration::from_secs(60);

pub async fn make(repo: &Repo, patch: Option<&str>, into: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(into)?;
    match &repo.fixture {
        Some(fixture) => copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture), into)?,
        None => export(repo, into).await?,
    }
    git(&["init", "-q"], into).await?;
    if let Some(patch) = patch {
        let file = bench().join("patches").join(patch);
        git(&["apply", &file.to_string_lossy()], into).await?;
    }
    git(&["add", "-A"], into).await?;
    git(&["commit", "-q", "-m", "Initial commit"], into).await?;
    if let Some(line) = &repo.prepare {
        let ran = cmd::sh(&repo.fill(line), into, &repo.env(), PREPARE_LIMIT).await;
        if !ran.ok {
            bail!("{}: prepare failed: {}", repo.id, ran.output.trim());
        }
    }
    Ok(())
}

/// The cache's checked-out commit, without its `.git`.
async fn export(repo: &Repo, into: &Path) -> anyhow::Result<()> {
    let cache = repo.cache();
    if !cache.join(".git").exists() {
        bail!("{} is not set up; run `make battery-setup`", repo.id);
    }
    let tar = into.with_extension("tar");
    git(
        &[
            "archive",
            "--format=tar",
            "-o",
            &tar.to_string_lossy(),
            "HEAD",
        ],
        &cache,
    )
    .await?;
    let out = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(&tar)
        .current_dir(into)
        .output()
        .await
        .context("tar")?;
    std::fs::remove_file(&tar)?;
    if !out.status.success() {
        bail!("tar: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| from.display().to_string())? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
