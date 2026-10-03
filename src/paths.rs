use std::path::{Path, PathBuf};

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

    /// Full tool output too long to show, kept for the model to read.
    pub fn tmp_dir(&self) -> PathBuf {
        self.home.join("tmp")
    }

    pub fn plans_dir(&self) -> PathBuf {
        self.home.join("plans")
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

    /// When the updater last looked, and news of an update for the next start.
    pub fn update_state(&self) -> PathBuf {
        self.home.join("update.json")
    }

    /// Grouped per project directory so resuming can list this project's sessions first.
    pub fn sessions_dir(&self) -> PathBuf {
        self.home.join("sessions").join(slug(&self.project))
    }
}

fn slug(path: &Path) -> String {
    let text: String = path
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    text.trim_matches('-').chars().take(96).collect()
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
