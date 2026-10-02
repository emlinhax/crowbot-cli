//! The real `crowbot` binary against an in-process fake crowbot; never the live API.
// Tests arrange the outside world directly; the io-wrapper rule is for the product code.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

mod chat;
mod fake_crowbot;
mod login;
mod models;
mod pty;
mod scenarios;
mod signup;

use std::path::{Path, PathBuf};
use std::process::Output;

use tempfile::TempDir;

/// A throwaway `CROWBOT_HOME` and project directory for one test.
pub struct Sandbox {
    pub home: TempDir,
    pub project: TempDir,
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

    /// Runs crowbot pointed at `api_url`, with `key` as its env key; with none it has only
    /// what the home stores.
    pub async fn run(&self, api_url: &str, args: &[&str], key: Option<&str>) -> Run {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_crowbot"));
        command
            .args(args)
            .current_dir(self.project.path())
            // An inherited stdin pipe that never closes would look like piped input forever.
            .stdin(std::process::Stdio::null());
        for name in CLEARED {
            command.env_remove(name);
        }
        command
            .envs(launch_env(self.home.path(), api_url, key))
            .kill_on_drop(true);
        let output = tokio::time::timeout(RUN_LIMIT, command.output())
            .await
            .unwrap_or_else(|_| panic!("crowbot {args:?} still running after {RUN_LIMIT:?}"));
        Run(output.expect("crowbot binary runs"))
    }

    /// Copies tests/fixtures/projects/<name> into the project directory.
    pub fn copy_project(&self, name: &str) {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects")
            .join(name);
        copy_dir(&from, self.project.path());
    }

    /// Every session file written so far.
    pub fn sessions(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let root = self.home.path().join("sessions");
        for project in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(project.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                found.push(file.path());
            }
        }
        found
    }
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

pub struct Run(pub Output);

impl Run {
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    pub fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    #[track_caller]
    pub fn success(&self) -> &Self {
        assert!(
            self.0.status.success(),
            "crowbot failed ({})\nstdout:\n{}\nstderr:\n{}",
            self.0.status,
            self.stdout(),
            self.stderr()
        );
        self
    }
}

/// An address nothing listens on, for offline behaviour.
pub const DEAD_URL: &str = "http://127.0.0.1:9";

/// How long one headless run may take before the test fails instead of hanging CI. The slowest
/// run today takes a few seconds; a full retry ladder without Retry-After is about 30 s.
const RUN_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Makes crowbot use the fake's accepted test key.
pub const WITH_KEY: Option<&str> = Some(fake_crowbot::ENV_KEY);

/// What the developer's shell must not bring into a test: their key, what picks colour depth,
/// glyphs or the clock (io/term.rs, io/clock.rs), so local and CI runs see the same, and their
/// desktop's clipboard (io/clipboard.rs).
pub const CLEARED: &[&str] = &[
    "CROWBOT_API_KEY",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "NO_COLOR",
    "COLORTERM",
    "WT_SESSION",
    "TERM_PROGRAM",
    "CROWBOT_FAKE_NOW",
];

/// What every crowbot a test starts is launched with, headless or in a terminal.
pub fn launch_env(home: &Path, api_url: &str, key: Option<&str>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("CROWBOT_HOME", home.display().to_string()),
        ("CROWBOT_API_URL", api_url.to_owned()),
        ("CROWBOT_CHAT_URL", api_url.to_owned()),
        ("CROWBOT_NO_BROWSER", "1".to_owned()),
    ];
    if let Some(key) = key {
        env.push(("CROWBOT_API_KEY", key.to_owned()));
    }
    env
}
