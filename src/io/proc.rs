//! Other programs and the machine we run on.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::{Path, PathBuf};

/// Opens `url` in the default browser. Best effort: callers always print the URL as well.
pub fn open_url(url: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        // ShellExecute, not `cmd /c start`, which splits URLs at `&`.
        let wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(Some(0))
                .collect()
        };
        let (verb, target) = (wide("open"), wide(url));
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute reports success as a value above 32.
        if result as usize > 32 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
    }
}

/// Runs `command` in cmd.exe once this process has had time to exit, detached from it; how a
/// program removes the folder it runs from.
#[cfg(windows)]
pub fn after_exit(command: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    std::process::Command::new("cmd")
        .raw_arg(format!("/c ping 127.0.0.1 -n 3 > nul & {command}"))
        .creation_flags(DETACHED_PROCESS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// The program this process runs from.
pub fn current_exe() -> io::Result<PathBuf> {
    std::env::current_exe()
}

/// The first `name` on PATH (trying PATHEXT extensions on Windows), or `name` itself when it is
/// already a path to a program.
pub fn which(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return runnable(direct).then(|| direct.to_path_buf());
    }
    let path = crate::settings::env("PATH")?;
    let exts: Vec<String> = if cfg!(windows) && direct.extension().is_none() {
        crate::settings::env("PATHEXT")
            .unwrap_or_else(|| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| runnable(candidate))
    })
}

/// A file this user may execute; on Windows the extension decides, so any file is.
fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// This machine's name, shown on crowbot's key list after pairing.
pub fn hostname() -> String {
    os_hostname()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(unix)]
fn os_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer outlives the call, which writes at most `buf.len()` bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).trim().to_owned())
}

#[cfg(not(unix))]
fn os_hostname() -> Option<String> {
    crate::settings::env("COMPUTERNAME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_has_a_name() {
        assert_ne!(hostname(), "unknown");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_file_that_may_run_is_a_program() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let name = tool.to_str().unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(which(name), None);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which(name), Some(tool.clone()));
        assert_eq!(which(dir.path().to_str().unwrap()), None);
    }
}
