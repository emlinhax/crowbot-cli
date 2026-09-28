use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shell::Shell;
use super::truncate::{self, Keep};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::io::shell::{self, Ended, ShellCommand};
use crate::io::{self, fs::Access};
use crate::limits;
use crate::permission::gate::Ask;
use crate::permission::{arity, shell_split};

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
        let mut asks = vec![Ask {
            permission: "bash".into(),
            always: split
                .commands
                .iter()
                .map(|c| arity::always_pattern(c))
                .collect(),
            patterns: split.commands,
        }];
        // What runs cannot be read off a substitution, and a redirect writes files: both get
        // their own permission so read-only allowances never cover them.
        if split.complex {
            asks.push(Ask::new("bash_complex", args.command.trim()));
        }
        if split.writes {
            asks.push(Ask::new("bash_write", args.command.trim()));
        }
        Ok(Check { asks })
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
            let run = shell::run(
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
                &mut |_| {},
            )
            .await;
            let run = match run {
                Ok(run) => run,
                Err(e) => {
                    return Output::error(format!("Could not start {}: {e}", self.shell.name));
                }
            };
            let text = clean(&String::from_utf8_lossy(&run.output));
            let cut = truncate::cut(
                &text,
                Keep::Tail,
                limits.max_lines.value,
                limits.max_bytes.value,
            );
            let mut content = String::new();
            let mut spill = None;
            if cut.truncated() {
                spill = save_full(cx, &text);
                let _ = writeln!(
                    content,
                    "[Showing the last {} of {} lines.{}]",
                    cut.kept_lines,
                    cut.total_lines,
                    spill
                        .as_ref()
                        .map(|p| format!(" Full output: {p}"))
                        .unwrap_or_default()
                );
            }
            content.push_str(&cut.text);
            let (status, failed, code) = match run.ended {
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

/// Drops terminal escapes and keeps only what a `\r`-redrawn progress line last showed.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() || n == '~' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                // OSC sequences end with BEL or ESC \.
                while let Some(n) = chars.next() {
                    if n == '\u{7}' || (n == '\u{1b}' && chars.peek() == Some(&'\\')) {
                        chars.next();
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out.lines()
        .map(|line| line.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn save_full(cx: &ToolCx<'_>, text: &str) -> Option<String> {
    let path = cx
        .app
        .paths
        .tmp_dir()
        .join(format!("bash-{:08x}.log", fastrand::u32(..)));
    io::fs::write_atomic(&path, text.as_bytes(), Access::Shared).ok()?;
    Some(path.display().to_string())
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
    fn asks_per_command_with_arity_suggestions() {
        let asks = asks("git status && cargo test --all");
        assert_eq!(asks[0].patterns, vec!["git status", "cargo test --all"]);
        assert_eq!(asks[0].always, vec!["git status", "cargo test *"]);
        assert_eq!(asks.len(), 1);
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
