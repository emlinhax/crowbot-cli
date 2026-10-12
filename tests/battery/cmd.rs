//! The commands the battery runs itself: git, and the repos' setup, prepare and check lines.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};

pub struct Ran {
    pub ok: bool,
    /// stdout, then stderr.
    pub output: String,
}

// CEILING: a line runs under `sh` and only `sh` is ended on a timeout; what it started may
// outlive it. Windows needs Git Bash's `sh` on PATH. Upgrade: run lines through crowbot's own
// process-group shell (src/io/shell.rs) once the battery can link against it.
pub async fn sh(line: &str, cwd: &Path, env: &[(String, String)], limit: Duration) -> Ran {
    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", line])
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, command.output()).await {
        Ok(Ok(out)) => Ran {
            ok: out.status.success(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        },
        Ok(Err(e)) => Ran {
            ok: false,
            output: format!("could not start sh: {e}"),
        },
        Err(_) => Ran {
            ok: false,
            output: format!("still running after {limit:?}"),
        },
    }
}

/// Runs git with an identity and no signing, so commits work on any machine; fails on a
/// non-zero exit.
pub async fn git(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("git")
        .args([
            "-c",
            "user.name=battery",
            "-c",
            "user.email=battery@localhost",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .await
        .context("git")?;
    if !out.status.success() {
        bail!(
            "git {} in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
