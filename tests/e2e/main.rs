//! The real `crowbot` binary against an in-process fake crowbot; never the live API.
// Tests arrange the outside world directly; the io-wrapper rule is for the product code.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

mod fake_crowbot;
mod models;

use std::path::Path;
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

    pub async fn run(&self, api_url: &str, args: &[&str]) -> Run {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_crowbot"))
            .args(args)
            .current_dir(self.project.path())
            .env("CROWBOT_HOME", self.home.path())
            .env("CROWBOT_API_URL", api_url)
            .env_remove("CROWBOT_API_KEY")
            .output()
            .await
            .expect("crowbot binary runs");
        Run(output)
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
