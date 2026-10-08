use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::limits;
use crate::io;

/// Every location crowbot reads or writes, derived from two roots.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.crowbot`, or `CROWBOT_HOME`.
    pub home: PathBuf,
    /// The directory crowbot was started in.
    pub project: PathBuf,
}

impl Paths {
    pub fn resolve() -> anyhow::Result<Self> {
        let home = match io::env::var("CROWBOT_HOME") {
            Some(home) => PathBuf::from(home),
            None => io::env::home_dir()
                .context("no home directory; set CROWBOT_HOME")?
                .join(".crowbot"),
        };
        let project = io::env::current_dir().context("reading the current directory")?;
        Ok(Self::at(home, project))
    }

    pub fn at(home: PathBuf, project: PathBuf) -> Self {
        Self { home, project }
    }

    pub fn user_config(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    pub fn project_config(&self) -> PathBuf {
        self.project.join(".crowbot").join("config.toml")
    }

    pub fn models_cache(&self) -> PathBuf {
        self.home.join("cache").join("models.json")
    }

    /// Full tool output too long to show, kept for the model to read.
    pub fn tmp_dir(&self) -> PathBuf {
        self.home.join("tmp")
    }

    /// Where PLAN mode writes this session's plan.
    pub fn plan_file(&self, session: &str) -> PathBuf {
        self.home.join("plans").join(format!("{session}.md"))
    }

    pub fn auth(&self) -> PathBuf {
        self.home.join("auth.json")
    }

    /// The forums the user added and the session cookies for the ones logged in.
    pub fn forums(&self) -> PathBuf {
        self.home.join("forums.json")
    }

    /// Project folders whose own config may loosen the settings.
    pub fn trusted(&self) -> PathBuf {
        self.home.join("trusted.json")
    }

    /// Whether the user said no to installing, so they are not asked again.
    pub fn install_state(&self) -> PathBuf {
        self.home.join("install.json")
    }

    /// When the updater last looked, and news of an update for the next start.
    pub fn update_state(&self) -> PathBuf {
        self.home.join("update.json")
    }

    /// One folder per project.
    pub fn sessions_dir(&self) -> PathBuf {
        self.home.join("sessions").join(slug(&self.project))
    }
}

// CEILING: keeping the head lets long paths that share it land in one folder. Keeping the tail, or
// adding a short hash, fixes that, and moves the folders of paths over the limit.
fn slug(path: &Path) -> String {
    let text: String = path
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let max = limits::get().session.slug_chars.value;
    text.trim_matches('-').chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(
            slug(Path::new(r"C:\Users\jacob\proj")),
            "C--Users-jacob-proj"
        );
        assert_eq!(slug(Path::new("/home/j/p.rs")), "home-j-p-rs");
    }
}
