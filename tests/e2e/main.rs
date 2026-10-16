//! The real `crowbot` binary against an in-process fake crowbot; never the live API.
// Tests arrange the outside world directly; the io-wrapper rule is for the product code.
#![allow(clippy::disallowed_methods)]

mod chat;
#[path = "../common/mod.rs"]
mod common;
mod fake_crowbot;
mod login;
mod models;
mod pty;
mod scenarios;
mod signup;

use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub use common::{Run, cleared, launch_env, tests_path};

/// A throwaway `CROWBOT_HOME` and project directory for one test.
pub struct Sandbox {
    home: TempDir,
    project: TempDir,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        }
    }
}

impl Sandbox {
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn project(&self) -> &Path {
        self.project.path()
    }

    /// Writes `text` at `rel` under the home, making its folders.
    pub fn write_home(&self, rel: &str, text: &str) {
        write(&self.home().join(rel), text);
    }

    /// Writes `text` at `rel` under the project, making its folders.
    pub fn write_project(&self, rel: &str, text: &str) {
        write(&self.project().join(rel), text);
    }

    /// Runs crowbot pointed at `api_url`, with `key` as its env key; with none it has only
    /// what the home stores.
    pub async fn run(&self, api_url: &str, args: &[&str], key: Option<&str>) -> Run {
        common::launch(self.home(), self.project(), args, Some(api_url), key).await
    }

    /// Copies tests/fixtures/projects/<name> into the project directory.
    pub fn copy_project(&self, name: &str) {
        copy_dir(&tests_path("fixtures/projects").join(name), self.project());
    }

    /// Every session file written so far; a subagent's, in a folder beside its parent's, are not.
    pub fn sessions(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let root = self.home.path().join("sessions");
        for project in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(project.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if file.path().extension().is_some_and(|e| e == "jsonl") {
                    found.push(file.path());
                }
            }
        }
        found
    }
}

/// Every file's text under `dir`, one after another, for what a run must not have written.
pub fn all_text(dir: &Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_text(&path));
        } else {
            out.push_str(&String::from_utf8_lossy(
                &std::fs::read(&path).unwrap_or_default(),
            ));
        }
    }
    out
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// An address nothing listens on, for offline behaviour.
pub const DEAD_URL: &str = "http://127.0.0.1:9";

/// Makes crowbot use the fake's accepted test key.
pub const WITH_KEY: Option<&str> = Some(fake_crowbot::ENV_KEY);
