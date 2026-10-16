//! git as the shadow runs it: its own git dir over the project's work tree, with config and
//! environment that keep the user's git settings out (data/snapshot.toml).

use std::path::{Path, PathBuf};

use crate::config::limits;
use crate::io;
use crate::io::proc::{Captured, Program};

use super::DATA;

pub struct Git {
    pub program: PathBuf,
    /// The shadow's git dir, under crowbot's home.
    pub dir: PathBuf,
    /// The work tree it keeps: the project's repository root.
    pub work: PathBuf,
}

impl Git {
    /// Runs `git <shadow options> <args>` in the work tree; a non-zero exit is an error that
    /// names what git said.
    pub async fn run(&self, args: &[String]) -> Result<Captured, String> {
        let mut all = options(Some(&self.dir));
        all.push(format!("--git-dir={}", self.dir.display()));
        all.push(format!("--work-tree={}", self.work.display()));
        all.extend_from_slice(args);
        exec(&self.program, &self.work, &all).await
    }
}

/// The work tree `project` is inside, as git sees it, or `None` outside one.
pub async fn toplevel(program: &Path, project: &Path) -> Option<PathBuf> {
    let mut args = options(None);
    args.extend(["rev-parse".to_owned(), "--show-toplevel".to_owned()]);
    let out = exec(program, project, &args).await.ok()?;
    let top = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// `-c` for each setting; with the shadow's git dir, hooks pointed at its own empty folder so no
/// hook of the user's runs on a restore.
fn options(dir: Option<&Path>) -> Vec<String> {
    let mut args = Vec::new();
    for setting in &DATA.config {
        args.push("-c".to_owned());
        args.push(setting.clone());
    }
    if let Some(dir) = dir {
        args.push("-c".to_owned());
        args.push(format!("core.hooksPath={}", dir.join("hooks").display()));
    }
    args.push("--literal-pathspecs".to_owned());
    args
}

async fn exec(program: &Path, cwd: &Path, args: &[String]) -> Result<Captured, String> {
    let env: Vec<(String, String)> = DATA
        .env
        .iter()
        .map(|[k, v]| (k.clone(), v.clone()))
        .collect();
    let out = io::proc::exec(&Program {
        program,
        args,
        cwd,
        env: &env,
        unset: &DATA.unset,
        timeout: limits::get().snapshot.timeout_secs.secs(),
    })
    .await
    .map_err(|e| e.to_string())?;
    if out.ok() {
        Ok(out)
    } else {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(said.lines().last().unwrap_or("git failed").to_owned())
    }
}
