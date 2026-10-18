//! The bash tool: runs a command in the user's shell and tells the model what it printed.

pub mod shell;
mod spill;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use shell::Shell;
use spill::{Sink, Spill};

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::config::limits;
use crate::data;
use crate::io::proc::{self, Ended, ShellCommand};
use crate::permission;
use crate::text::controls;
use crate::text::template::fill;
use crate::text::truncate::{self, Keep};

static SPEC: LazyLock<Spec> = crate::tools::spec!("bash");

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

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if args.command.trim().is_empty() {
            return Err(Refusal::InvalidArgs(TEXT.empty.clone()));
        }
        // A shell reader asks as the read tool does of each file it names.
        let read = |file: &str| {
            let target = target::resolve(&cx.app.paths, file).ok()?;
            Some(target::asks(permission::catalog::READ.name, &target))
        };
        let asks = permission::bash::asks(&args.command, &cx.app.paths.project, &read);
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
            // Where the last call left the shell (a `cd` that stuck), else the project root.
            let cwd = cx
                .shared
                .cwd()
                .unwrap_or_else(|| cx.app.paths.project.clone());
            let mut sink = Sink::new(cx.app.paths.tmp_dir());
            let ended = proc::run(
                &ShellCommand {
                    program,
                    args: &self.shell.args,
                    command: &args.command,
                    cwd: &cwd,
                    env: &self.shell.env,
                    unset: &self.shell.unset,
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
            // A leading `cd` carries to the next call, so the model need not repeat it.
            if let Some(next) = next_cwd(&args.command, &cwd) {
                cx.shared.set_cwd(next);
            }
            Ok(self.report(&sink, ended, secs, &cx.app.paths.tmp_dir()))
        })
    }
}

impl Bash {
    /// What the model reads: the tail of what was printed, a note when that is not all of it,
    /// and how the command ended.
    fn report(&self, sink: &Sink, ended: Ended, secs: u64, spill_dir: &Path) -> Output {
        let limits = &limits::get().tools;
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
                // The whole output is still held; it goes to a file in the same raw form a
                // streamed spill has.
                spill = Spill::open(spill_dir, &sink.held).map(|s| s.path.display().to_string());
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
        out.with_details(json!({"exit_code": code, "full_output": spill, "shell": self.shell.name}))
    }
}

/// Where a `cd` as the command's first statement leaves the shell, so the next call starts there.
/// CEILING: only a leading, unquoted `cd <dir>`; a `cd` later in the command, or a path with spaces
/// or shell expansion, is not tracked — the model just repeats it, as it did before.
fn next_cwd(command: &str, current: &Path) -> Option<PathBuf> {
    let first = permission::bash::split::split(command)
        .commands
        .into_iter()
        .next()?;
    let mut words = first.split_whitespace();
    if words.next()? != "cd" {
        return None;
    }
    let target = words.next().filter(|w| !w.starts_with('-'))?;
    if words.next().is_some() {
        return None;
    }
    let joined = match Path::new(target).is_absolute() {
        true => PathBuf::from(target),
        false => current.join(target),
    };
    let path = crate::io::fs::canonical(&joined);
    crate::io::fs::is_dir(&path).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::gate::Ask;
    use crate::tools::testing::Project;

    fn asks(project: &Project, command: &str) -> Vec<Ask> {
        project
            .check(
                &Bash::new(shell::resolve(None)),
                &json!({ "command": command }),
            )
            .unwrap()
            .asks
    }

    #[test]
    fn a_leading_cd_moves_the_working_directory_for_the_next_call() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub");
        crate::io::fs::make_dir(&sub).unwrap();
        let canon = crate::io::fs::canonical(&sub);
        // Absolute and relative `cd` both land on the subdir.
        assert_eq!(
            next_cwd(&format!("cd {} && ls", sub.display()), root.path()),
            Some(canon.clone())
        );
        assert_eq!(next_cwd("cd sub && ls -la", root.path()), Some(canon));
        // No `cd`, a missing dir, or a non-plain `cd` leave it unchanged.
        assert_eq!(next_cwd("objdump -d x", root.path()), None);
        assert_eq!(next_cwd("cd nope && x", root.path()), None);
        assert_eq!(next_cwd("cd -", root.path()), None);
    }

    #[test]
    fn naming_the_project_as_the_directory_asks_nothing_of_its_own() {
        let project = Project::new();
        let dir = project.app.paths.project.display().to_string();
        let found = asks(
            &project,
            &format!("cd '{dir}' && git -C '{dir}' log -3 && cargo test"),
        );
        assert_eq!(found[0].patterns, ["git log -3", "cargo test"]);
        assert!(asks(&project, &format!("cd '{dir}'")).is_empty());
    }

    #[test]
    fn a_reader_asks_as_a_read_of_each_file_it_names() {
        let project = Project::new();
        assert_eq!(
            asks(&project, "grep -n exactly_n more.py | head -5"),
            [Ask::new("read", "more.py")]
        );
        assert_eq!(
            asks(&project, "cat .env && cargo test"),
            [Ask::new("read", ".env"), Ask::new("bash", "cargo test")]
        );
        let outside = asks(&project, "cat /etc/hosts");
        assert_eq!(outside[0].permission, "read");
        assert_eq!(outside[1].permission, "external_directory");
        assert_eq!(
            asks(&project, "cd src && cat a.rs")[0].patterns,
            ["cd src", "cat a.rs"],
            "past a cd, a.rs is not the project's"
        );
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
        let spilled = crate::io::fs::read_string(std::path::Path::new(&spill))
            .unwrap()
            .unwrap();
        assert_eq!(spilled.lines().count(), 200_000);
        assert!(out.content.len() <= limits::get().tools.max_bytes.value + 200);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_cut_that_stayed_in_memory_saves_the_raw_output_as_a_spill_does() {
        let project = Project::new();
        let bash = Bash::new(shell::resolve(None));
        let lines = limits::get().tools.max_lines.value + 10;
        let command = format!("printf '\\033[31mred\\033[0m\\n'; seq 1 {lines}");
        let out = project.run(&bash, json!({ "command": command })).await;
        assert!(!out.content.contains('\x1b'), "{}", out.content);
        let file = out.details.as_ref().unwrap()["full_output"]
            .as_str()
            .unwrap()
            .to_owned();
        let saved = crate::io::fs::read_string(Path::new(&file))
            .unwrap()
            .unwrap();
        assert!(saved.starts_with("\x1b[31mred\x1b[0m\n"), "{saved:?}");
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
