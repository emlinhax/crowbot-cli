//! Running a command line so that nothing it starts outlives it: a Job Object on Windows,
//! a process group on Unix.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::group::{self, Tree};

pub struct ShellCommand<'a> {
    pub program: &'a Path,
    /// Arguments before the command line, e.g. `-c`.
    pub args: &'a [String],
    pub command: &'a str,
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    /// Inherited variables the command must not see.
    pub unset: &'a [String],
    pub timeout: Duration,
    /// How long output may keep arriving after the shell itself exits.
    pub drain: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

/// Output goes to `on_output` as it arrives (stdout and stderr interleaved) and is not kept
/// here, so a command that prints forever costs the caller only what it chooses to keep.
pub async fn run(
    cmd: &ShellCommand<'_>,
    cancel: &CancellationToken,
    on_output: &mut (dyn FnMut(&[u8]) + Send),
) -> io::Result<Ended> {
    let mut command = tokio::process::Command::new(cmd.program);
    command
        .args(cmd.args)
        .arg(cmd.command)
        .current_dir(cmd.cwd)
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in cmd.unset {
        command.env_remove(name);
    }
    group::contain(&mut command);

    let mut child = command.spawn()?;
    let tree = Tree::adopt(&child)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, tx));
    }

    let deadline = tokio::time::sleep(cmd.timeout);
    tokio::pin!(deadline);
    let ended = loop {
        tokio::select! {
            Some(chunk) = rx.recv() => on_output(&chunk),
            status = child.wait() => break Ended::Exited(status.ok().and_then(|s| s.code())),
            () = &mut deadline => break Ended::TimedOut,
            () = cancel.cancelled() => break Ended::Cancelled,
        }
    };
    // What the command left running ends with it, so the pipes close once their last writer is
    // gone and what is still in them is read to the end, however slow the reader (a loaded
    // Windows); `drain` bounds only a process that escaped the tree.
    // CEILING: background processes (dev servers) die with the command; running one detached
    // would need its own lifecycle and output channel.
    tree.kill();
    let drain = tokio::time::sleep(cmd.drain);
    tokio::pin!(drain);
    loop {
        tokio::select! {
            chunk = rx.recv() => match chunk {
                Some(chunk) => on_output(&chunk),
                None => break,
            },
            () = &mut drain => break,
        }
    }
    Ok(ended)
}

async fn pump(mut from: impl tokio::io::AsyncRead + Unpin, to: mpsc::UnboundedSender<Vec<u8>>) {
    let mut buf = vec![0u8; 8192];
    while let Ok(n) = from.read(&mut buf).await {
        if n == 0 || to.send(buf[..n].to_vec()).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> (std::path::PathBuf, Vec<String>) {
        if cfg!(windows) {
            (
                crate::io::proc::which("powershell.exe").expect("powershell on PATH"),
                vec!["-NoProfile".into(), "-Command".into()],
            )
        } else {
            ("/bin/sh".into(), vec!["-c".into()])
        }
    }

    /// What the line printed, and how it ended.
    async fn run_line(
        line: &str,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> (Vec<u8>, Ended) {
        run_line_unset(line, timeout, cancel, &[]).await
    }

    async fn run_line_unset(
        line: &str,
        timeout: Duration,
        cancel: &CancellationToken,
        unset: &[String],
    ) -> (Vec<u8>, Ended) {
        let (program, args) = shell();
        let cwd = std::env::temp_dir();
        let mut output = Vec::new();
        let ended = run(
            &ShellCommand {
                program: &program,
                args: &args,
                command: line,
                cwd: &cwd,
                env: &[],
                unset,
                timeout,
                drain: Duration::from_secs(2),
            },
            cancel,
            &mut |chunk| output.extend_from_slice(chunk),
        )
        .await
        .unwrap();
        (output, ended)
    }

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let (output, ended) = run_line(
            "echo hello; exit 3",
            Duration::from_secs(20),
            &CancellationToken::new(),
        )
        .await;
        assert!(String::from_utf8_lossy(&output).contains("hello"));
        assert_eq!(ended, Ended::Exited(Some(3)));
    }

    #[tokio::test]
    async fn an_unset_variable_is_not_inherited() {
        // One every test process inherits on its OS, standing in for crowbot's key.
        let (name, line) = if cfg!(windows) {
            (
                "USERPROFILE",
                "if ($env:USERPROFILE) { 'inherited' } else { 'gone' }",
            )
        } else {
            ("HOME", "echo \"${HOME-gone}\"")
        };
        assert!(std::env::var_os(name).is_some(), "{name} is set here");
        let (output, _) = run_line_unset(
            line,
            Duration::from_secs(20),
            &CancellationToken::new(),
            &[name.to_owned()],
        )
        .await;
        assert_eq!(String::from_utf8_lossy(&output).trim(), "gone");
    }

    #[tokio::test]
    async fn times_out_and_kills_the_tree() {
        let started = std::time::Instant::now();
        let sleep = if cfg!(windows) {
            "Start-Sleep 30"
        } else {
            "sleep 30"
        };
        let (_, ended) =
            run_line(sleep, Duration::from_millis(500), &CancellationToken::new()).await;
        assert_eq!(ended, Ended::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn cancellation_stops_the_command() {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            trigger.cancel();
        });
        let sleep = if cfg!(windows) {
            "Start-Sleep 30"
        } else {
            "sleep 30"
        };
        let (_, ended) = run_line(sleep, Duration::from_secs(60), &cancel).await;
        assert_eq!(ended, Ended::Cancelled);
    }
}
