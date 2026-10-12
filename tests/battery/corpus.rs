//! bench/: the run's settings, the repositories and the tasks, each read from its file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub fn bench() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("bench")
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub model: String,
    pub repeats: u32,
    pub parallel: usize,
    pub cap_micros: Limit,
    pub run_cap_micros: Limit,
    pub run_wall_secs: Limit,
    pub stop_grace_secs: Limit,
    pub check_secs: Limit,
}

impl Settings {
    pub fn limits(&self) -> [(&'static str, &Limit); 5] {
        [
            ("cap_micros", &self.cap_micros),
            ("run_cap_micros", &self.run_cap_micros),
            ("run_wall_secs", &self.run_wall_secs),
            ("stop_grace_secs", &self.stop_grace_secs),
            ("check_secs", &self.check_secs),
        ]
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub value: u64,
    pub reason: String,
}

impl Limit {
    pub fn secs(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.value)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Repos {
    repo: Vec<Repo>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub id: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    /// A directory of this checkout to copy instead of an outside repo.
    #[serde(default)]
    pub fixture: Option<String>,
    /// Run once in the cache after cloning, e.g. to fetch dependencies.
    #[serde(default)]
    pub setup: Option<String>,
    /// Run in each run's copy after its one commit.
    #[serde(default)]
    pub prepare: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The repo's own suite; it must pass on an untouched copy.
    pub test: String,
}

impl Repo {
    pub fn cache(&self) -> PathBuf {
        bench().join("cache").join(&self.id)
    }

    pub fn fill(&self, text: &str) -> String {
        text.replace("{cache}", &slashes(&self.cache()))
    }

    pub fn env(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(k, v)| (k.clone(), self.fill(v)))
            .collect()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// The file's name without `.toml`.
    #[serde(skip)]
    pub id: String,
    pub prompt: String,
    pub repo: String,
    pub mode: String,
    /// Model turns before the run is stopped.
    pub turns: u32,
    /// Exit 0 means the task was done; see `run::check_line` for its placeholders.
    pub check: String,
    /// A file in bench/patches/ applied before the copy's one commit.
    #[serde(default)]
    pub patch: Option<String>,
}

pub fn settings() -> Settings {
    parse(&bench().join("battery.toml"))
}

pub fn repos() -> Vec<Repo> {
    parse::<Repos>(&bench().join("repos.toml")).repo
}

/// Every task, sorted by id.
pub fn tasks() -> Vec<Task> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(bench().join("tasks"))
        .expect("bench/tasks")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| Task {
            id: path.file_stem().unwrap().to_string_lossy().into_owned(),
            ..parse(path)
        })
        .collect()
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A path as a shell command or a permission rule spells it.
pub fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
