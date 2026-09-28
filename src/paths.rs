use std::path::PathBuf;

use anyhow::Context;

use crate::settings;

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
        let home = match settings::env("CROWBOT_HOME") {
            Some(home) => PathBuf::from(home),
            None => dirs::home_dir()
                .context("no home directory; set CROWBOT_HOME")?
                .join(".crowbot"),
        };
        let project = std::env::current_dir().context("reading the current directory")?;
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
}
