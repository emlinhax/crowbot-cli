//! Running a command line so that nothing it starts outlives it: a Job Object on Windows,
//! a process group on Unix.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct ShellCommand<'a> {
    pub program: &'a Path,
    /// Arguments before the command line, e.g. `-c`.
    pub args: &'a [String],
    pub command: &'a str,
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
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
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
        // No console window, and our own Ctrl+C is not delivered to the child.
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    // CEILING: a child that calls setsid or runs under `set -m` leaves the group and survives;
    // closing that needs PR_SET_CHILD_SUBREAPER or a cgroup.
    #[cfg(unix)]
    command.process_group(0);

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
    if !matches!(ended, Ended::Exited(_)) {
        tree.kill();
    }
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
    // CEILING: background processes (dev servers) die with the command; running one detached
    // would need its own lifecycle and output channel.
    tree.kill();
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

/// Every process a command started. Dropping it kills them all.
struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    group: i32,
}

// SAFETY: the job handle is only used through thread-safe Win32 calls.
#[cfg(windows)]
unsafe impl Send for Tree {}

impl Tree {
    #[cfg(windows)]
    fn adopt(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls on a handle we own; the struct is zeroed as the API expects.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            // From here every early return closes the job, and the caller's `?` drops the
            // child, which `kill_on_drop` ends: nothing runs untracked.
            let tree = Self { job };
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            // CEILING: a grandchild spawned before this line escapes the job; closing that gap
            // needs CREATE_SUSPENDED and resuming the main thread after assignment.
            let handle = child.raw_handle().ok_or_else(untracked)?;
            if AssignProcessToJobObject(job, handle) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(tree)
        }
    }

    #[cfg(unix)]
    fn adopt(child: &tokio::process::Child) -> io::Result<Self> {
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or_else(untracked)?;
        Ok(Self { group })
    }

    fn kill(&self) {
        #[cfg(windows)]
        // SAFETY: the job handle stays valid until drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        #[cfg(unix)]
        // SAFETY: signalling a process group we created; failure means it is already gone.
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
    }
}

fn untracked() -> io::Error {
    io::Error::other("the command exited before it could be tracked")
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
        #[cfg(windows)]
        // SAFETY: closing the handle we created, once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
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
                timeout,
                drain: Duration::from_millis(200),
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
