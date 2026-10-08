use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::shell::Shell;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::config::limits;
use crate::data;
use crate::io::proc::{self, Ended, ShellCommand};
use crate::io::{self, fs::Access};
use crate::permission;
use crate::text::controls;
use crate::text::template::fill;
use crate::text::truncate::{self, Keep};

static SPEC: LazyLock<Spec> = super::spec!("bash");

static TEXT: LazyLock<Text> = data::load!("tools/bash.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    empty: String,
    no_start: String,
    tail: String,
    full: String,
    stops: String,
    exit_code: String,
    signal: String,
    timed_out: String,
    cancelled: String,
    no_output: String,
}

#[derive(Deserialize)]
pub struct Args {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

pub struct Bash {
    shell: Shell,
}

impl Bash {
    pub fn new(shell: Shell) -> Self {
        Self { shell }
    }
}

impl Tool for Bash {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if args.command.trim().is_empty() {
            return Err(Refusal::InvalidArgs(TEXT.empty.clone()));
        }
        let asks = permission::bash::asks(&args.command);
        Ok(Check::new(asks).with_preview(format!("$ {}", args.command.trim())))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let Some(program) = &self.shell.program else {
                return Err(self.shell.note.clone());
            };
            let limits = &limits::get().tools;
            let secs = args
                .timeout
                .unwrap_or(limits.bash_timeout_secs.value)
                .clamp(1, limits.bash_max_timeout_secs.value);
            let mut sink = Sink::new(cx.app.paths.tmp_dir());
            let ended = proc::run(
                &ShellCommand {
                    program,
                    args: &self.shell.args,
                    command: &args.command,
                    cwd: &cx.app.paths.project,
                    env: &self.shell.env,
                    timeout: Duration::from_secs(secs),
                    drain: limits.bash_drain_ms.ms(),
                },
                &cx.cancel,
                &mut |chunk| sink.take(chunk),
            )
            .await
            .map_err(|e| {
                fill(
                    &TEXT.no_start,
                    &[("shell", &self.shell.name), ("error", &e.to_string())],
                )
            })?;
            let text = controls::as_printed(&String::from_utf8_lossy(&sink.held));
            let cut = truncate::cut(
                &text,
                Keep::Tail,
                limits.max_lines.value,
                limits.max_bytes.value,
            );
            let mut content = String::new();
            let mut spill = sink.spill.as_ref().map(|s| s.path.display().to_string());
            if cut.truncated() || spill.is_some() {
                let total = if spill.is_some() {
                    sink.lines()
                } else {
                    cut.total_lines
                };
                if spill.is_none() {
                    spill = save_full(cx, &text);
                }
                let stops = if sink.spill.as_ref().is_some_and(|s| s.stopped) {
                    let bytes = limits.bash_spill_max_bytes.value.to_string();
                    fill(&TEXT.stops, &[("bytes", &bytes)])
                } else {
                    String::new()
                };
                let full = spill
                    .as_ref()
                    .map(|p| fill(&TEXT.full, &[("path", p)]))
                    .unwrap_or_default();
                content.push_str(&fill(
                    &TEXT.tail,
                    &[
                        ("kept", &cut.kept_lines.to_string()),
                        ("total", &total.to_string()),
                        ("full", &full),
                        ("stops", &stops),
                    ],
                ));
                content.push('\n');
            }
            content.push_str(&cut.text);
            let (status, failed, code) = match ended {
                Ended::Exited(Some(0)) => (String::new(), false, Some(0)),
                Ended::Exited(Some(code)) => (
                    fill(&TEXT.exit_code, &[("code", &code.to_string())]),
                    true,
                    Some(code),
                ),
                Ended::Exited(None) => (TEXT.signal.clone(), true, None),
                Ended::TimedOut => (
                    fill(&TEXT.timed_out, &[("secs", &secs.to_string())]),
                    true,
                    None,
                ),
                Ended::Cancelled => (TEXT.cancelled.clone(), true, None),
            };
            if !status.is_empty() {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&status);
            }
            if content.is_empty() {
                content = TEXT.no_output.clone();
            }
            let out = if failed {
                Output::error(content)
            } else {
                Output::ok(content)
            };
            Ok(out.with_details(
                json!({"exit_code": code, "full_output": spill, "shell": self.shell.name}),
            ))
        })
    }
}

/// What a command printed, within bounds: all of it while small, then only the latest part in
/// memory and the whole, up to a cap, in a spill file the model can read.
struct Sink {
    held: Vec<u8>,
    spill: Option<Spill>,
    spill_dir: std::path::PathBuf,
    newlines: usize,
    ends_mid_line: bool,
}

struct Spill {
    file: io::fs::Appender,
    path: std::path::PathBuf,
    written: usize,
    stopped: bool,
}

impl Sink {
    fn new(spill_dir: std::path::PathBuf) -> Self {
        Self {
            held: Vec::new(),
            spill: None,
            spill_dir,
            newlines: 0,
            ends_mid_line: false,
        }
    }

    fn take(&mut self, chunk: &[u8]) {
        self.newlines += chunk.iter().filter(|&&b| b == b'\n').count();
        if let Some(&last) = chunk.last() {
            self.ends_mid_line = last != b'\n';
        }
        if let Some(spill) = &mut self.spill {
            spill.write(chunk);
        }
        self.held.extend_from_slice(chunk);
        let keep = limits::get().tools.bash_memory_bytes.value;
        if self.held.len() > keep {
            if self.spill.is_none() {
                self.spill = Spill::open(&self.spill_dir, &self.held);
            }
            let excess = self.held.len() - keep;
            self.held.drain(..excess);
        }
    }

    fn lines(&self) -> usize {
        self.newlines + usize::from(self.ends_mid_line)
    }
}

impl Spill {
    fn open(dir: &std::path::Path, start: &[u8]) -> Option<Self> {
        let (path, file) = create_spill(dir)?;
        let mut spill = Self {
            file,
            path,
            written: 0,
            stopped: false,
        };
        spill.write(start);
        Some(spill)
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.stopped {
            return;
        }
        let room = limits::get().tools.bash_spill_max_bytes.value - self.written;
        let part = &bytes[..bytes.len().min(room)];
        self.stopped = part.len() < bytes.len() || self.file.append(part).is_err();
        self.written += part.len();
    }
}

fn save_full(cx: &ToolCx<'_>, text: &str) -> Option<String> {
    let (path, mut file) = create_spill(&cx.app.paths.tmp_dir())?;
    file.append(text.as_bytes()).ok()?;
    Some(path.display().to_string())
}

static PRUNED: std::sync::Once = std::sync::Once::new();

/// A new spill file, never one an older transcript still points to. The process's first spill
/// also clears out old ones.
fn create_spill(dir: &std::path::Path) -> Option<(std::path::PathBuf, io::fs::Appender)> {
    PRUNED.call_once(|| prune_spills(dir));
    // 32 random bits rarely collide; a few draws make it never in practice.
    for _ in 0..8 {
        let path = dir.join(format!("bash-{:08x}.log", fastrand::u32(..)));
        match io::fs::Appender::create(&path, Access::Shared) {
            Ok(file) => return Some((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

fn prune_spills(dir: &std::path::Path) {
    let keep = limits::get().tools.bash_spill_keep_days.signed_days();
    let now = io::clock::now();
    for entry in io::fs::list_dir(dir).unwrap_or_default() {
        if !(entry.name.starts_with("bash-") && entry.name.ends_with(".log")) {
            continue;
        }
        let path = dir.join(&entry.name);
        let age = io::fs::modified(&path)
            .and_then(|m| jiff::Timestamp::try_from(m).ok())
            .map(|m| now.duration_since(m));
        if age.is_some_and(|age| age > keep) {
            let _ = io::fs::remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::shell;
    use crate::tools::testing::Project;

    // Ageing a file is something only this test does, so it reaches the filesystem itself.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn old_spills_are_pruned_and_fresh_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (old, fresh, other) = ("bash-00000001.log", "bash-00000002.log", "notes.log");
        for name in [old, fresh, other] {
            io::fs::write_atomic(&dir.path().join(name), b"x", Access::Shared).unwrap();
        }
        for name in [old, other] {
            std::fs::File::options()
                .write(true)
                .open(dir.path().join(name))
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000))
                .unwrap();
        }
        prune_spills(dir.path());
        let left: Vec<String> = io::fs::list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(left, vec![fresh, other]);
    }

    #[test]
    fn a_spill_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let names: std::collections::HashSet<_> = (0..50)
            .map(|_| create_spill(dir.path()).unwrap().0)
            .collect();
        assert_eq!(names.len(), 50);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_flood_of_output_is_held_in_bounds_and_spilled_whole() {
        let project = Project::new();
        let bash = Bash::new(shell::resolve(None));
        let out = project.run(&bash, json!({"command": "seq 1 200000"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("of 200000 lines"), "{}", out.content);
        assert!(out.content.ends_with("200000"), "{}", out.content);
        let spill = out.details.as_ref().unwrap()["full_output"]
            .as_str()
            .unwrap()
            .to_owned();
        let spilled = io::fs::read_string(std::path::Path::new(&spill))
            .unwrap()
            .unwrap();
        assert_eq!(spilled.lines().count(), 200_000);
        assert!(out.content.len() <= limits::get().tools.max_bytes.value + 200);
    }

    #[tokio::test]
    async fn runs_in_the_project_and_reports_failures() {
        let project = Project::new();
        project.write("marker.txt", "here");
        let bash = Bash::new(shell::resolve(None));
        let ok = project.run(&bash, json!({"command": "ls"})).await;
        assert!(ok.content.contains("marker.txt"), "{}", ok.content);
        let bad = project.run(&bash, json!({"command": "exit 7"})).await;
        assert!(bad.is_error);
        assert!(bad.content.contains("exit code 7"), "{}", bad.content);
    }
}
