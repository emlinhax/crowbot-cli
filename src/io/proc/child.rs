//! A program that keeps running beside crowbot and talks in lines on stdin and stdout (an MCP
//! server): started in a process group of its own, ended with crowbot or when stopped.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

use super::group::{self, Tree};

pub struct Service<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    /// Inherited variables it must not see; one `env` sets is set all the same.
    pub unset: &'a [String],
    /// The longest line it may print; a longer one ends what is read from it.
    pub line_bytes: usize,
    /// How much of what it prints on stderr is kept, from the end, to say why it failed.
    pub stderr_bytes: usize,
}

/// A started program: the lines it prints, a way to send it lines, and the process itself.
pub struct Running {
    /// Each line it prints; an `Err` names why no more will come (a line past the limit, a read
    /// that failed). The channel closes when it closes stdout.
    pub incoming: mpsc::UnboundedReceiver<Result<String, String>>,
    /// Lines to send it; dropping every sender, or `Process::stop`, closes its stdin.
    pub outgoing: mpsc::UnboundedSender<String>,
    pub process: Process,
}

pub struct Process {
    child: tokio::process::Child,
    tree: Tree,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    /// Dropped to close its stdin, however many senders are still about.
    stdin_open: oneshot::Sender<()>,
}

pub fn start(service: &Service<'_>) -> io::Result<Running> {
    let mut command = tokio::process::Command::new(service.program);
    command
        .args(service.args)
        .current_dir(service.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in service.unset {
        command.env_remove(name);
    }
    command.envs(service.env.iter().map(|(k, v)| (k, v)));
    group::contain(&mut command);
    let mut child = command.spawn()?;
    let tree = Tree::adopt(&child)?;

    let (tx, incoming) = mpsc::unbounded_channel();
    if let Some(stdout) = child.stdout.take() {
        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(service.line_bytes));
        tokio::spawn(read_lines(lines, tx));
    }
    let (outgoing, rx) = mpsc::unbounded_channel::<String>();
    let (stdin_open, closed) = oneshot::channel();
    if let Some(stdin) = child.stdin.take() {
        tokio::spawn(write_lines(stdin, rx, closed));
    }
    let stderr = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(err) = child.stderr.take() {
        tokio::spawn(keep_tail(err, stderr.clone(), service.stderr_bytes));
    }
    Ok(Running {
        incoming,
        outgoing,
        process: Process {
            child,
            tree,
            stderr,
            stdin_open,
        },
    })
}

impl Process {
    /// The end of what it printed on stderr, as text.
    pub fn stderr_tail(&self) -> String {
        let kept = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        let bytes: Vec<u8> = kept.iter().copied().collect();
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }

    /// Closes its stdin, waits up to `grace` for it to end on its own, then ends it and all it
    /// started.
    pub async fn stop(self, grace: Duration) {
        let Self {
            mut child,
            tree,
            stdin_open,
            ..
        } = self;
        drop(stdin_open);
        let _ = crate::io::clock::timeout(grace, child.wait()).await;
        tree.kill();
    }
}

async fn read_lines(
    mut lines: FramedRead<tokio::process::ChildStdout, LinesCodec>,
    to: mpsc::UnboundedSender<Result<String, String>>,
) {
    while let Some(line) = lines.next().await {
        let ended = line.is_err();
        if to.send(line.map_err(|e| e.to_string())).is_err() || ended {
            break;
        }
    }
}

async fn write_lines(
    mut stdin: tokio::process::ChildStdin,
    mut from: mpsc::UnboundedReceiver<String>,
    mut closed: oneshot::Receiver<()>,
) {
    loop {
        let line = tokio::select! {
            line = from.recv() => line,
            _ = &mut closed => None,
        };
        let Some(mut line) = line else { break };
        line.push('\n');
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn keep_tail(
    mut from: tokio::process::ChildStderr,
    into: Arc<Mutex<VecDeque<u8>>>,
    cap: usize,
) {
    let mut buf = vec![0u8; 4096];
    while let Ok(n) = from.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let mut kept = into.lock().unwrap_or_else(|e| e.into_inner());
        kept.extend(&buf[..n]);
        let over = kept.len().saturating_sub(cap);
        kept.drain(..over);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell that echoes each line back, says `bye` on stderr when stdin closes, and exits.
    fn echo(sandbox: &Path) -> (std::path::PathBuf, Vec<String>) {
        if cfg!(windows) {
            let program = crate::io::proc::which("powershell.exe").expect("powershell on PATH");
            let script = "while (($l = [Console]::In.ReadLine()) -ne $null) { [Console]::Out.WriteLine($l); [Console]::Out.Flush() }; [Console]::Error.WriteLine('bye')";
            (
                program,
                vec!["-NoProfile".into(), "-Command".into(), script.into()],
            )
        } else {
            let _ = sandbox;
            let script = "while IFS= read -r l; do printf '%s\\n' \"$l\"; done; echo bye >&2";
            ("/bin/sh".into(), vec!["-c".into(), script.into()])
        }
    }

    fn service<'a>(
        program: &'a Path,
        args: &'a [String],
        cwd: &'a Path,
        line_bytes: usize,
    ) -> Service<'a> {
        Service {
            program,
            args,
            cwd,
            env: &[],
            unset: &[],
            line_bytes,
            stderr_bytes: 2,
        }
    }

    #[tokio::test]
    async fn lines_go_both_ways_and_the_end_of_stderr_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = echo(dir.path());
        let mut running = start(&service(&program, &args, dir.path(), 1024)).unwrap();
        running.outgoing.send("hello".into()).unwrap();
        assert_eq!(running.incoming.recv().await, Some(Ok("hello".into())));
        drop(running.outgoing);
        assert_eq!(running.incoming.recv().await, None);
        running.process.stop(Duration::from_secs(5)).await;
    }

    #[tokio::test]
    async fn a_stop_closes_stdin_while_a_sender_lives_on() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = echo(dir.path());
        let running = start(&service(&program, &args, dir.path(), 1024)).unwrap();
        let grace = Duration::from_secs(10);
        let started = std::time::Instant::now();
        running.process.stop(grace).await;
        assert!(started.elapsed() < grace, "it waited out the grace");
        drop(running.outgoing);
    }

    #[tokio::test]
    async fn a_line_past_the_limit_ends_what_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = echo(dir.path());
        let mut running = start(&service(&program, &args, dir.path(), 8)).unwrap();
        running
            .outgoing
            .send("far longer than eight".into())
            .unwrap();
        assert!(matches!(running.incoming.recv().await, Some(Err(_))));
        assert_eq!(running.incoming.recv().await, None);
    }

    #[tokio::test]
    async fn only_the_tail_of_stderr_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = echo(dir.path());
        let running = start(&service(&program, &args, dir.path(), 1024)).unwrap();
        let Running {
            outgoing,
            mut incoming,
            process,
        } = running;
        drop(outgoing);
        while incoming.recv().await.is_some() {}
        crate::io::clock::sleep(Duration::from_millis(200)).await;
        let tail = process.stderr_tail();
        assert!("bye".ends_with(&tail) && tail.len() <= 2, "{tail:?}");
    }
}
