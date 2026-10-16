//! The project's files as they were before a prompt changed them, kept in a shadow git under
//! crowbot's home so `/undo` can put them back (data/snapshot.toml). Only inside a git work tree:
//! its .gitignore keeps build output and dependencies out of the copy.
//! CEILING: the work tree only (not what bash writes elsewhere, nor nested repositories), one
//! shadow index shared by sessions in the same project, and nothing is ever pruned. Upgrade: a
//! per-session index file, and `git gc --prune` once a day.

mod git;

use std::sync::{LazyLock, Mutex};

use serde::Deserialize;

use crate::config::limits;
use crate::config::paths::Paths;
use crate::data;
use crate::io;
use crate::text::template::fill;
use git::Git;

static DATA: LazyLock<Data> = data::load!("snapshot.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    candidates: Vec<String>,
    config: Vec<String>,
    env: Vec<[String; 2]>,
    unset: Vec<String>,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    no_git: String,
    not_a_repo: String,
    failed: String,
}

/// The shadow for one session's project, or why there is none.
pub struct Shadow {
    git: Option<Git>,
    /// Why snapshots are off: never on, or stopped after a failure.
    off: Mutex<Option<String>>,
}

/// What a restore put back.
#[derive(Debug, Default)]
pub struct Restored {
    pub files: usize,
    /// Files changed again since the run ended, so someone else's work: left as they are.
    pub kept: Vec<String>,
    /// Each file it could not, with why.
    pub failed: Vec<String>,
}

/// One file a snapshot pair differs in.
struct Change {
    path: String,
    /// The later snapshot added it, so putting things back removes it.
    added: bool,
}

impl Shadow {
    /// The project's shadow; off when git is missing or the project is no git work tree.
    pub async fn open(paths: &Paths) -> Self {
        let Some(program) = io::proc::find(&DATA.candidates) else {
            return Self::none(DATA.text.no_git.clone());
        };
        let Some(work) = git::toplevel(&program, &paths.project).await else {
            let project = paths.project.display().to_string();
            return Self::none(fill(&DATA.text.not_a_repo, &[("project", &project)]));
        };
        let git = Git {
            dir: paths.snapshots_dir(&work),
            work,
            program,
        };
        let shadow = Self {
            git: Some(git),
            off: Mutex::new(None),
        };
        if let Err(e) = shadow.prepare(paths).await {
            shadow.stop(&e);
        }
        shadow
    }

    /// A shadow that never snapshots, saying `why`.
    pub fn none(why: String) -> Self {
        Self {
            git: None,
            off: Mutex::new(Some(why)),
        }
    }

    /// Why snapshots are off, if they are.
    pub fn off(&self) -> Option<String> {
        self.off.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The work tree as it is now, as a tree id; `None` while off. A failure turns snapshots off
    /// for the session.
    pub async fn take(&self) -> Option<String> {
        let git = self.on()?;
        let taken = async {
            git.run(&args(&["add", "-A"])).await?;
            let out = git.run(&args(&["write-tree"])).await?;
            Ok::<_, String>(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        match taken.await {
            Ok(tree) => Some(tree),
            Err(e) => {
                self.stop(&e);
                None
            }
        }
    }

    /// Puts every file that differs between snapshots `before` and `after` back as it was in
    /// `before`, removing those `after` added. A file changed again since `after` is someone
    /// else's work now and is left alone; without `after` (a run that never ended) every file
    /// that differs from now goes back.
    pub async fn restore(&self, before: &str, after: Option<&str>) -> Result<Restored, String> {
        let git = self.on().ok_or_else(|| self.off().unwrap_or_default())?;
        let now = self
            .take()
            .await
            .ok_or_else(|| self.off().unwrap_or_default())?;
        let (changes, since) = match after {
            Some(after) => (
                changes(git, before, after).await?,
                changes(git, after, &now).await?,
            ),
            None => (changes(git, before, &now).await?, Vec::new()),
        };
        let mut restored = Restored::default();
        let mut back = Vec::new();
        for change in changes {
            if since.iter().any(|c| c.path == change.path) {
                restored.kept.push(change.path);
            } else if change.added {
                match io::fs::remove(&git.work.join(&change.path)) {
                    Ok(_) => restored.files += 1,
                    Err(e) => restored.failed.push(format!("{}: {e}", change.path)),
                }
            } else {
                back.push(change.path);
            }
        }
        for chunk in chunks(&back) {
            let mut call = args(&["checkout", before, "--"]);
            call.extend(chunk.iter().cloned());
            match git.run(&call).await {
                Ok(_) => restored.files += chunk.len(),
                Err(e) => restored
                    .failed
                    .extend(chunk.iter().map(|path| format!("{path}: {e}"))),
            }
        }
        Ok(restored)
    }

    fn on(&self) -> Option<&Git> {
        self.git.as_ref().filter(|_| self.off().is_none())
    }

    fn stop(&self, error: &str) {
        let why = fill(&DATA.text.failed, &[("error", error)]);
        *self.off.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
    }

    /// Creates the shadow on first use, keeping crowbot's own home out of it when that lies
    /// inside the work tree.
    async fn prepare(&self, paths: &Paths) -> Result<(), String> {
        let Some(git) = &self.git else {
            return Ok(());
        };
        if !io::fs::exists(&git.dir.join("HEAD")) {
            io::fs::make_dir(&git.dir).map_err(|e| e.to_string())?;
            git.run(&args(&["init", "--quiet"])).await?;
        }
        // git names the work tree resolved (macOS's /private/var, Windows' long names), which
        // the home as configured may not be.
        let (home, work) = (io::fs::canonical(&paths.home), io::fs::canonical(&git.work));
        if let Ok(home) = home.strip_prefix(&work) {
            let exclude = format!("/{}/\n", io::fs::slashed(home));
            io::fs::write_atomic(
                &git.dir.join("info").join("exclude"),
                exclude.as_bytes(),
                io::fs::Access::Private,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// Files that differ between trees `a` and `b`.
async fn changes(git: &Git, a: &str, b: &str) -> Result<Vec<Change>, String> {
    let out = git
        .run(&args(&[
            "diff-tree",
            "-r",
            "--no-renames",
            "--name-status",
            "-z",
            a,
            b,
        ]))
        .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        changes.push(Change {
            path: path.to_owned(),
            added: status.starts_with('A'),
        });
    }
    Ok(changes)
}

/// `paths` in runs that fit one git command line (`snapshot.args_bytes`).
fn chunks(paths: &[String]) -> Vec<&[String]> {
    let budget = limits::get().snapshot.args_bytes.value;
    let mut out = Vec::new();
    let (mut start, mut used) = (0, 0);
    for (i, path) in paths.iter().enumerate() {
        if i > start && used + path.len() + 1 > budget {
            out.push(&paths[start..i]);
            (start, used) = (i, 0);
        }
        used += path.len() + 1;
    }
    if start < paths.len() {
        out.push(&paths[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project that is a git work tree, with crowbot's home inside it as the tests have it;
    /// `None` on a machine without git.
    async fn project() -> Option<(tempfile::TempDir, Paths)> {
        let program = io::proc::find(&DATA.candidates)?;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        io::fs::make_dir(&project).unwrap();
        let init = io::proc::exec(&io::proc::Program {
            program: &program,
            args: &args(&["init", "--quiet"]),
            cwd: &project,
            env: &[],
            unset: &[],
            timeout: std::time::Duration::from_secs(30),
        })
        .await
        .unwrap();
        assert!(init.ok());
        let paths = Paths::at(project.join(".crowbot-home"), project);
        Some((dir, paths))
    }

    fn write(paths: &Paths, name: &str, bytes: &[u8]) {
        io::fs::write_atomic(&paths.project.join(name), bytes, io::fs::Access::Shared).unwrap();
    }

    fn read(paths: &Paths, name: &str) -> Option<Vec<u8>> {
        io::fs::read_bytes(&paths.project.join(name)).ok()
    }

    #[tokio::test]
    async fn a_restore_puts_back_changed_and_deleted_files_and_removes_added_ones() {
        let Some((_dir, paths)) = project().await else {
            return;
        };
        write(&paths, "kept.txt", b"line one\r\nline two\r\n");
        write(&paths, "with space [x].txt", b"odd name");
        write(&paths, "gone.txt", b"to be deleted");
        let shadow = Shadow::open(&paths).await;
        assert_eq!(shadow.off(), None);
        let before = shadow.take().await.unwrap();

        write(&paths, "kept.txt", b"changed");
        write(&paths, "with space [x].txt", b"changed too");
        write(&paths, "new.txt", b"added");
        io::fs::remove(&paths.project.join("gone.txt")).unwrap();
        // The home inside the project is never part of a snapshot.
        io::fs::make_dir(&paths.home).unwrap();
        io::fs::write_atomic(
            &paths.home.join("auth.json"),
            b"{}",
            io::fs::Access::Private,
        )
        .unwrap();
        let after = shadow.take().await.unwrap();

        // Someone edits one of them after the run: theirs now, so it stays.
        write(&paths, "with space [x].txt", b"edited by hand");
        let restored = shadow.restore(&before, Some(&after)).await.unwrap();
        assert_eq!((restored.files, restored.failed.len()), (3, 0));
        assert_eq!(restored.kept, ["with space [x].txt"]);
        assert_eq!(
            read(&paths, "kept.txt").unwrap(),
            b"line one\r\nline two\r\n"
        );
        assert_eq!(
            read(&paths, "with space [x].txt").unwrap(),
            b"edited by hand"
        );
        assert_eq!(read(&paths, "gone.txt").unwrap(), b"to be deleted");
        assert_eq!(read(&paths, "new.txt"), None);
        assert!(io::fs::exists(&paths.home.join("auth.json")));
        // Keyed by the work tree as git names it, which a link or short name can differ from.
        let shadow_dir = &shadow.git.as_ref().unwrap().dir;
        assert!(io::fs::exists(&shadow_dir.join("info/exclude")));
    }

    /// macOS's temp folders sit behind a link (/var is /private/var) and Windows' behind short
    /// names, while git names the work tree resolved: crowbot's home inside it stays out all
    /// the same.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_project_reached_through_a_link_still_keeps_the_home_out() {
        let Some((dir, real)) = project().await else {
            return;
        };
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real.project, &link).unwrap();
        let paths = Paths::at(link.join(".crowbot-home"), link);
        write(&paths, "a.txt", b"one");
        let shadow = Shadow::open(&paths).await;
        let before = shadow.take().await.unwrap();
        io::fs::make_dir(&paths.home).unwrap();
        io::fs::write_atomic(
            &paths.home.join("auth.json"),
            b"{}",
            io::fs::Access::Private,
        )
        .unwrap();
        write(&paths, "a.txt", b"two");
        let after = shadow.take().await.unwrap();
        let restored = shadow.restore(&before, Some(&after)).await.unwrap();
        assert_eq!(
            (restored.files, restored.failed.len()),
            (1, 0),
            "{restored:?}"
        );
        assert!(io::fs::exists(&paths.home.join("auth.json")));
    }

    #[tokio::test]
    async fn outside_a_git_work_tree_there_are_no_snapshots_and_it_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("plain"));
        io::fs::make_dir(&paths.project).unwrap();
        let shadow = Shadow::open(&paths).await;
        assert!(shadow.off().is_some());
        assert_eq!(shadow.take().await, None);
    }

    #[test]
    fn paths_are_split_to_fit_a_command_line() {
        let budget = limits::get().snapshot.args_bytes.value;
        let long = "x".repeat(budget / 3);
        let paths = vec![long.clone(), long.clone(), long.clone(), "y".into()];
        let runs = chunks(&paths);
        assert_eq!(runs.iter().map(|r| r.len()).sum::<usize>(), 4);
        assert!(runs.len() >= 2);
        assert!(chunks(&[]).is_empty());
    }
}
