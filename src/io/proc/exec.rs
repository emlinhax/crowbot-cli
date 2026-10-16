//! Running a helper crowbot itself needs (git, for snapshots) to its end and keeping what it
//! printed. The model's commands go through tree.rs, which streams and owns their process tree.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

pub struct Program<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    /// Inherited variables it must not see.
    pub unset: &'a [String],
    pub timeout: Duration,
}

pub struct Captured {
    /// `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Captured {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs `p` with no input; past its timeout it is killed and this is a `TimedOut` error.
pub async fn exec(p: &Program<'_>) -> io::Result<Captured> {
    let mut command = tokio::process::Command::new(p.program);
    command
        .args(p.args)
        .current_dir(p.cwd)
        .envs(p.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in p.unset {
        command.env_remove(name);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match tokio::time::timeout(p.timeout, command.output()).await {
        Ok(output) => output.map(|o| Captured {
            code: o.status.code(),
            stdout: o.stdout,
            stderr: o.stderr,
        }),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{} took longer than {:?}", p.program.display(), p.timeout),
        )),
    }
}
