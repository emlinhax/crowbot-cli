//! One task run: crowbot headless in a fresh copy and home, watched until it ends or a limit
//! stops it, then checked and recorded.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::cmd::{self, git};
use crate::corpus::{Repo, Settings, Task, bench, slashes};
use crate::project;
use crate::record::{self, Record};

/// Where crowbot is pointed, and with which key.
pub struct Target {
    /// `None` is the live API.
    pub api_url: Option<String>,
    pub key: String,
}

/// What every run of one battery shares.
pub struct Battery {
    pub settings: Settings,
    pub target: Target,
    /// A models cache each run's home starts with, so the model resolves without a fetch.
    pub models: Option<Vec<u8>>,
    /// Where run directories and results.jsonl go.
    pub out: PathBuf,
    spent: AtomicU64,
    results: Mutex<()>,
}

impl Battery {
    pub fn new(settings: Settings, target: Target, models: Option<Vec<u8>>, out: PathBuf) -> Self {
        Self {
            settings,
            target,
            models,
            out,
            spent: AtomicU64::new(0),
            results: Mutex::new(()),
        }
    }

    pub fn spent(&self) -> u64 {
        self.spent.load(Ordering::Relaxed)
    }

    /// Once the cap is reached no run starts and the running ones are stopped.
    pub fn capped(&self) -> bool {
        self.spent() >= self.settings.cap_micros.value
    }

    /// Runs one task and appends its line to results.jsonl, whatever happened.
    pub async fn run(&self, repo: &Repo, task: &Task, rep: u32) -> Record {
        let record = match one(self, repo, task, rep).await {
            Ok(record) => record,
            Err(e) => Record {
                task: task.id.clone(),
                rep,
                error: Some(format!("{e:#}")),
                ..Record::default()
            },
        };
        let _one_writer = self.results.lock().unwrap();
        if let Err(e) = record::append(&self.out.join("results.jsonl"), &record) {
            eprintln!("battery: results.jsonl: {e:#}");
        }
        record
    }
}

async fn one(battery: &Battery, repo: &Repo, task: &Task, rep: u32) -> anyhow::Result<Record> {
    let settings = &battery.settings;
    let key = &battery.target.key;
    let home = tempfile::tempdir()?;
    let parent = tempfile::tempdir()?;
    let copy = parent.path().join(&repo.id);
    project::make(repo, task.patch.as_deref(), &copy).await?;
    if let Some(models) = &battery.models {
        let cache = home.path().join("cache");
        std::fs::create_dir_all(&cache)?;
        std::fs::write(cache.join("models.json"), models)?;
    }
    // The runs keep the real HOME for the repos' builds; the MCP servers its apps list stay out of
    // what is measured.
    std::fs::write(home.path().join("config.toml"), "mcp_discovery = false\n")?;

    let watched = watch(battery, repo, task, home.path(), &copy).await?;

    let rel = format!("{}/{rep}", task.id);
    let dir = battery.out.join(&rel);
    std::fs::create_dir_all(&dir)?;
    // Taken before the check, which may add files of its own.
    git(&["add", "-A"], &copy).await?;
    let diff = git(&["diff", "--cached"], &copy).await?;
    let session = session_file(home.path()).context("crowbot wrote no session file")?;
    let stem = session.file_stem().unwrap().to_string_lossy().into_owned();
    // Spelled as crowbot's tools resolve it: links followed, `/` separators.
    let plan = std::fs::canonicalize(home.path())?
        .join("plans")
        .join(format!("{stem}.md"));
    let reply = dir.join("reply.txt");
    record::save(&dir, "reply.txt", &watched.reply, key)?;
    let check = cmd::sh(
        &check_line(&task.check, &reply, &plan),
        &copy,
        &repo.env(),
        settings.check_secs.secs(),
    )
    .await;

    record::save(
        &dir,
        "session.jsonl",
        &std::fs::read_to_string(&session)?,
        key,
    )?;
    record::save(&dir, "events.jsonl", &watched.events, key)?;
    record::save(&dir, "stderr.txt", &watched.stderr, key)?;
    record::save(&dir, "diff.patch", &diff, key)?;
    record::save(&dir, "check.txt", &check.output, key)?;
    if let Ok(text) = std::fs::read_to_string(&plan) {
        record::save(&dir, "plan.md", &text, key)?;
    }
    Ok(Record {
        task: task.id.clone(),
        repo: repo.id.clone(),
        mode: task.mode.clone(),
        model: settings.model.clone(),
        rep,
        exit: watched.exit,
        stopped_by: watched.stopped_by,
        check: check.ok,
        turns: watched.turns,
        cost_micros: watched.cost,
        wall_ms: u64::try_from(watched.wall.as_millis()).unwrap_or(u64::MAX),
        plan_file: slashes(&plan),
        dir: rel,
        error: None,
    })
}

/// A task's check line with its placeholders filled, each quoted for `sh`: `{reply}` is a file
/// holding the run's last reply, `{plan}` its plan file (absent when it wrote none) and
/// `{checks}` bench/checks/.
pub fn check_line(check: &str, reply: &Path, plan: &Path) -> String {
    check
        .replace("{reply}", &quote(reply))
        .replace("{plan}", &quote(plan))
        .replace("{checks}", &quote(&bench().join("checks")))
}

pub const PLACEHOLDERS: &[&str] = &["{reply}", "{plan}", "{checks}"];

fn quote(path: &Path) -> String {
    format!("'{}'", slashes(path).replace('\'', r"'\''"))
}

fn session_file(home: &Path) -> Option<PathBuf> {
    let projects = std::fs::read_dir(home.join("sessions")).ok()?;
    projects
        .flatten()
        .flat_map(|p| std::fs::read_dir(p.path()).into_iter().flatten().flatten())
        .map(|f| f.path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
}

#[derive(Default)]
struct Watched {
    exit: Option<i32>,
    stopped_by: Option<String>,
    /// The last reply ended the run on its own, so a limit reached with it stops nothing.
    finished: bool,
    turns: u32,
    cost: u64,
    wall: Duration,
    /// Every `--json` line, each with the milliseconds since start it arrived at.
    events: String,
    stderr: String,
    reply: String,
}

async fn watch(
    battery: &Battery,
    repo: &Repo,
    task: &Task,
    home: &Path,
    copy: &Path,
) -> anyhow::Result<Watched> {
    let settings = &battery.settings;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_crowbot"));
    command
        .args(["--model", &settings.model, "--mode", &task.mode, "--json"])
        .arg(&task.prompt)
        .current_dir(copy)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    child_env(&mut command, &battery.target, home);
    command.envs(repo.env());

    let started = Instant::now();
    let mut child = command.spawn().context("crowbot")?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });
    let mut lines = BufReader::new(stdout).lines();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut w = Watched::default();
    let mut kill_at = None;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => tally(&mut w, battery, &line, started),
                _ => break,
            },
            _ = tick.tick() => {}
        }
        if w.stopped_by.is_none() {
            w.stopped_by = limit_reached(&w, battery, task, started.elapsed()).map(str::to_owned);
            if w.stopped_by.is_some() {
                interrupt(&mut child);
                kill_at = Some(Instant::now() + settings.stop_grace_secs.secs());
            }
        }
        if kill_at.is_some_and(|at| Instant::now() >= at) {
            let _ = child.start_kill();
        }
    }
    let status = match tokio::time::timeout(settings.stop_grace_secs.secs(), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.start_kill();
            child.wait().await?
        }
    };
    w.exit = status.code();
    w.wall = started.elapsed();
    w.stderr = errors.await.unwrap_or_default();
    Ok(w)
}

fn tally(w: &mut Watched, battery: &Battery, line: &str, started: Instant) {
    let event: Value = serde_json::from_str(line).unwrap_or_else(|_| Value::from(line));
    // A subagent's and a summary's requests are spent too; only the run's own replies are turns.
    let cost = cost(&event);
    w.cost += cost;
    battery.spent.fetch_add(cost, Ordering::Relaxed);
    if event["type"] == "message_end" {
        let message = &event["message"];
        w.turns += 1;
        w.finished = message["finish"] == "done";
        let text: String = message["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect();
        if !text.trim().is_empty() {
            w.reply = text;
        }
    }
    let ms = started.elapsed().as_millis();
    w.events
        .push_str(&json!({ "ms": ms, "event": event }).to_string());
    w.events.push('\n');
}

/// What a stream event spent: a reply's or a summary's cost, however deep the subagent.
fn cost(event: &Value) -> u64 {
    match event["type"].as_str() {
        Some("message_end") => event["message"]["usage"]["cost_micros"]
            .as_u64()
            .unwrap_or(0),
        Some("compaction_end") => event["usage"]["cost_micros"].as_u64().unwrap_or(0),
        Some("child") => cost(&event["event"]),
        _ => 0,
    }
}

fn limit_reached(
    w: &Watched,
    battery: &Battery,
    task: &Task,
    elapsed: Duration,
) -> Option<&'static str> {
    let settings = &battery.settings;
    if battery.capped() {
        Some("cap")
    } else if w.turns >= task.turns && !w.finished {
        Some("turns")
    } else if w.cost >= settings.run_cap_micros.value {
        Some("run_cap")
    } else if elapsed >= settings.run_wall_secs.secs() {
        Some("wall")
    } else {
        None
    }
}

/// Ctrl-C, which crowbot answers by ending its commands' process groups and then itself;
/// killing it outright would leave builds and servers running.
fn interrupt(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) {
        // SAFETY: signals a child this process started and has not yet reaped.
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
    }
    // A console-less child gets no Ctrl-C on Windows; killing crowbot closes its Job Object,
    // which ends its commands with it.
    #[cfg(windows)]
    let _ = child.start_kill();
}

/// crowbot's environment: this process's, minus anything that would point it elsewhere or let
/// cargo's own variables leak into the repo's builds, plus the run's home and key.
/// `RUSTUP_TOOLCHAIN` stays, so a machine with no default toolchain still builds the Rust repos.
pub fn child_env(command: &mut tokio::process::Command, target: &Target, home: &Path) {
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        let cargo = name == "CARGO" || (name.starts_with("CARGO_") && name != "CARGO_HOME");
        if cargo || name.starts_with("CROWBOT_") {
            command.env_remove(name.as_ref());
        }
    }
    command
        .env("CROWBOT_HOME", home)
        .env("CROWBOT_API_KEY", &target.key)
        .env("CROWBOT_NO_BROWSER", "1");
    if let Some(url) = &target.api_url {
        command
            .env("CROWBOT_API_URL", url)
            .env("CROWBOT_CHAT_URL", url);
    }
}
