use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::permissions;
use super::shell::Shell;
use super::truncate::{self, Keep};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::io::shell::{self, Ended, ShellCommand};
use crate::io::{self, fs::Access};
use crate::limits;
use crate::permission::gate::Ask;
use crate::permission::{filter, shell_split};
use crate::text::controls;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "bash",
        include_str!("../../data/tools/bash.md"),
        include_str!("../../data/tools/bash.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
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
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        if args.command.trim().is_empty() {
            return Err(Refusal::InvalidArgs("`command` is empty".into()));
        }
        let split = shell_split::split(&args.command);
        // A filter at the end of a pipe (`| tail -30`) reads only what the command before it
        // writes, so it asks for nothing of its own while something feeds it.
        let fed: Vec<String> = split
            .commands
            .iter()
            .filter(|c| !filter::reads_only_input(c))
            .cloned()
            .collect();
        let patterns = if fed.is_empty() { split.commands } else { fed };
        let mut asks = vec![Ask {
            permission: permissions::BASH.name.into(),
            patterns,
        }];
        // What runs cannot be read off a substitution, and a redirect writes files: both get
        // their own permission so read-only allowances never cover them.
        if split.complex {
            asks.push(Ask::new(
                permissions::BASH_COMPLEX.name,
                args.command.trim(),
            ));
        }
        if split.writes {
            asks.push(Ask::new(permissions::BASH_WRITE.name, args.command.trim()));
        }
        Ok(Check::new(asks).with_preview(format!("$ {}", args.command.trim())))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let Some(program) = &self.shell.program else {
                return Output::error(self.shell.note.clone());
            };
            let limits = &limits::get().tools;
            let secs = args
                .timeout
                .unwrap_or(limits.bash_timeout_secs.value)
                .clamp(1, limits.bash_max_timeout_secs.value);
            let mut sink = Sink::new(cx.app.paths.tmp_dir());
            let ended = shell::run(
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
            .await;
            let ended = match ended {
                Ok(ended) => ended,
                Err(e) => {
                    return Output::error(format!("Could not start {}: {e}", self.shell.name));
                }
            };
            let text = clean(&String::from_utf8_lossy(&sink.held));
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
                let stopped = if sink.spill.as_ref().is_some_and(|s| s.stopped) {
                    format!(" It stops at {} bytes.", limits.bash_spill_max_bytes.value)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    content,
                    "[Showing the last {} of {total} lines.{}{stopped}]",
                    cut.kept_lines,
                    spill
                        .as_ref()
                        .map(|p| format!(" Full output: {p}"))
                        .unwrap_or_default()
                );
            }
            content.push_str(&cut.text);
            let (status, failed, code) = match ended {
                Ended::Exited(Some(0)) => (String::new(), false, Some(0)),
                Ended::Exited(Some(code)) => (format!("[exit code {code}]"), true, Some(code)),
                Ended::Exited(None) => ("[terminated by a signal]".into(), true, None),
                Ended::TimedOut => (format!("[timed out after {secs}s; stopped]"), true, None),
                Ended::Cancelled => ("[cancelled by the user]".into(), true, None),
            };
            if !status.is_empty() {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&status);
            }
            if content.is_empty() {
                content = "(no output)".into();
            }
            let out = if failed {
                Output::error(content)
            } else {
                Output::ok(content)
            };
            out.with_details(
                json!({"exit_code": code, "full_output": spill, "shell": self.shell.name}),
            )
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

/// Drops terminal escapes and keeps only what a `\r`-redrawn progress line last showed.
fn clean(text: &str) -> String {
    controls::strip_escapes(text)
        .lines()
        .map(|line| line.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
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
    let days = limits::get().tools.bash_spill_keep_days.value;
    let keep = jiff::SignedDuration::from_hours(i64::try_from(days * 24).unwrap_or(i64::MAX));
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

    fn asks(command: &str) -> Vec<Ask> {
        let project = Project::new();
        Bash::new(shell::resolve(None))
            .check(&json!({ "command": command }), &project.cx())
            .unwrap()
            .asks
    }

    #[test]
    fn asks_once_per_command() {
        let asks = asks("git status && cargo test --all");
        assert_eq!(asks[0].patterns, vec!["git status", "cargo test --all"]);
        assert_eq!(asks.len(), 1);
    }

    #[test]
    fn a_pipe_filter_asks_nothing_while_something_feeds_it() {
        let patterns = |command: &str| asks(command).remove(0).patterns;
        assert_eq!(
            patterns("cargo test 2>&1 | grep -E 'test result|FAILED' | tail -5"),
            ["cargo test 2>&1"]
        );
        assert_eq!(
            patterns("cargo test | tail .env"),
            ["cargo test", "tail .env"]
        );
        assert_eq!(patterns("tail -5"), ["tail -5"]);
    }

    #[test]
    fn substitutions_and_redirects_ask_separately() {
        let perms: Vec<String> = asks("echo $(date) > out.txt")
            .into_iter()
            .map(|a| a.permission)
            .collect();
        assert_eq!(perms, vec!["bash", "bash_complex", "bash_write"]);
    }

    #[test]
    fn cleans_escapes_and_carriage_returns() {
        assert_eq!(clean("\u{1b}[31mred\u{1b}[0m\n10%\r50%\r100%"), "red\n100%");
    }

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
        let out = bash
            .run(json!({"command": "seq 1 200000"}), &project.cx())
            .await;
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
        let ok = bash.run(json!({"command": "ls"}), &project.cx()).await;
        assert!(ok.content.contains("marker.txt"), "{}", ok.content);
        let bad = bash.run(json!({"command": "exit 7"}), &project.cx()).await;
        assert!(bad.is_error);
        assert!(bad.content.contains("exit code 7"), "{}", bad.content);
    }
}
