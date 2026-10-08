//! How both test crates launch crowbot: what the shell must not bring in, what it is pointed at,
//! a time bound, and what the run left on stdout and stderr.
// Compiled into each test crate, which uses only its share.
#![allow(dead_code)]

use std::path::Path;
use std::process::Output;
use std::time::Duration;

/// How long one headless run may take before the test fails instead of hanging. The slowest e2e
/// run takes a few seconds; a full retry ladder without Retry-After is about 30 s.
pub const RUN_LIMIT: Duration = Duration::from_secs(60);

/// Developer env a test must not inherit: the key and API addresses, what picks colour, glyphs or
/// the clock (io/term.rs, io/clock.rs), and the clipboard (io/clipboard.rs). Terminals' own
/// variables come from data/terminals.toml, so a terminal added there is cleared too.
pub fn cleared() -> Vec<String> {
    let mut names: Vec<String> = [
        "CROWBOT_API_KEY",
        "CROWBOT_API_URL",
        "CROWBOT_CHAT_URL",
        "CROWBOT_FAKE_NOW",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "NO_COLOR",
        "COLORTERM",
        "TERM_PROGRAM",
    ]
    .map(str::to_owned)
    .into();
    let terminals: toml::Table =
        toml::from_str(include_str!("../../data/terminals.toml")).expect("terminals.toml parses");
    for table in terminals.values().filter_map(toml::Value::as_table) {
        let env = table.get("env").and_then(toml::Value::as_array);
        names.extend(
            env.into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(str::to_owned),
        );
    }
    names.sort();
    names.dedup();
    names
}

/// What every crowbot a test starts is launched with: its home, the API at `api_url` (the live
/// one when `None`), and `key` as its env key (only what the home stores when `None`).
pub fn launch_env(
    home: &Path,
    api_url: Option<&str>,
    key: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("CROWBOT_HOME", home.display().to_string()),
        ("CROWBOT_NO_BROWSER", "1".to_owned()),
    ];
    if let Some(url) = api_url {
        env.push(("CROWBOT_API_URL", url.to_owned()));
        env.push(("CROWBOT_CHAT_URL", url.to_owned()));
    }
    if let Some(key) = key {
        env.push(("CROWBOT_API_KEY", key.to_owned()));
    }
    env
}

/// Runs crowbot headless in `project` with `args`, within `RUN_LIMIT`.
pub async fn launch(
    home: &Path,
    project: &Path,
    args: &[&str],
    api_url: Option<&str>,
    key: Option<&str>,
) -> Run {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_crowbot"));
    command
        .args(args)
        .current_dir(project)
        // An inherited stdin pipe that never closes would look like piped input forever.
        .stdin(std::process::Stdio::null());
    for name in cleared() {
        command.env_remove(name);
    }
    command
        .envs(launch_env(home, api_url, key))
        .kill_on_drop(true);
    let output = tokio::time::timeout(RUN_LIMIT, command.output())
        .await
        .unwrap_or_else(|_| panic!("crowbot {args:?} still running after {RUN_LIMIT:?}"));
    Run(output.expect("crowbot binary runs"))
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

    /// The `--json` event stream, one value per line.
    pub fn events(&self) -> Vec<serde_json::Value> {
        self.stdout()
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is a JSON event"))
            .collect()
    }

    #[track_caller]
    pub fn stdout_has(&self, text: &str) -> &Self {
        let stdout = self.stdout();
        assert!(stdout.contains(text), "stdout lacks {text:?}:\n{stdout}");
        self
    }

    #[track_caller]
    pub fn stderr_has(&self, text: &str) -> &Self {
        let stderr = self.stderr();
        assert!(stderr.contains(text), "stderr lacks {text:?}:\n{stderr}");
        self
    }
}
