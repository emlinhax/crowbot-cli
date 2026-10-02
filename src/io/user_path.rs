//! The user's PATH and, on Windows, the Apps & Features entry. Windows keeps both in the registry
//! under HKCU, and crowbot edits them there. Elsewhere PATH belongs to the shell's profile, which
//! crowbot only reads.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::Path;

/// How a platform writes PATH: the separator, and whether case tells entries apart.
#[derive(Clone, Copy)]
struct Style {
    sep: char,
    ignore_case: bool,
}

const HERE: Style = if cfg!(windows) {
    Style {
        sep: ';',
        ignore_case: true,
    }
} else {
    Style {
        sep: ':',
        ignore_case: false,
    }
};

/// What adding a folder to PATH came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Already,
    /// Not on it, and only the user's shell profile can put it there.
    ShellProfile,
}

/// Puts `dir` on the user's PATH for terminals opened from now on.
pub fn add(dir: &Path) -> io::Result<Change> {
    match with_dir(&sys::path()?, &dir.to_string_lossy(), HERE) {
        None => Ok(Change::Already),
        Some(path) if sys::WRITES_PATH => sys::set_path(&path).map(|()| Change::Added),
        Some(_) => Ok(Change::ShellProfile),
    }
}

/// Takes `dir` off the user's PATH; `false` when crowbot did not have it there.
pub fn remove(dir: &Path) -> io::Result<bool> {
    match without_dir(&sys::path()?, &dir.to_string_lossy(), HERE) {
        Some(path) if sys::WRITES_PATH => sys::set_path(&path).map(|()| true),
        _ => Ok(false),
    }
}

/// The Apps & Features entry under `key`: string values, then numbers. Elsewhere there is none.
pub fn register_app(
    key: &str,
    strings: &[(&str, &str)],
    numbers: &[(&str, u32)],
) -> io::Result<()> {
    sys::register(key, strings, numbers)
}

pub fn unregister_app(key: &str) -> io::Result<()> {
    sys::unregister(key)
}

/// `path` with `dir` at its end, or `None` when `dir` is already on it.
fn with_dir(path: &str, dir: &str, style: Style) -> Option<String> {
    let entries: Vec<&str> = entries(path, style).collect();
    if entries.iter().any(|e| same(e, dir, style)) {
        return None;
    }
    let joined: Vec<&str> = entries.into_iter().chain([dir]).collect();
    Some(joined.join(&style.sep.to_string()))
}

/// `path` without `dir`, or `None` when `dir` was not on it.
fn without_dir(path: &str, dir: &str, style: Style) -> Option<String> {
    let entries: Vec<&str> = entries(path, style).collect();
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|e| !same(e, dir, style))
        .collect();
    (kept.len() < entries.len()).then(|| kept.join(&style.sep.to_string()))
}

fn entries(path: &str, style: Style) -> impl Iterator<Item = &str> {
    path.split(style.sep).filter(|e| !e.trim().is_empty())
}

/// The same folder, however its trailing separator or (on Windows) its case is written.
fn same(a: &str, b: &str, style: Style) -> bool {
    let trim = |s: &str| s.trim().trim_end_matches(['/', '\\']).to_owned();
    let (a, b) = (trim(a), trim(b));
    if style.ignore_case {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

#[cfg(windows)]
mod sys {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
        REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
        RegDeleteTreeW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    pub const WRITES_PATH: bool = true;
    const ENVIRONMENT: &str = "Environment";
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn check(code: u32) -> io::Result<()> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code as i32))
        }
    }

    /// An open HKCU key, closed on drop.
    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the key was opened by `open` and is closed once.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open(sub: &str) -> io::Result<Key> {
        let sub = wide(sub);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `sub` is NUL-terminated and outlives the call; `key` receives the handle.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        })?;
        Ok(Key(key))
    }

    fn read_string(key: &Key, name: &str) -> io::Result<Option<String>> {
        let name = wide(name);
        let (mut kind, mut size) = (0u32, 0u32);
        // SAFETY: a size query: no buffer, `size` receives the byte count.
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` bytes and outlives the call.
        check(unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut bytes,
            )
        })?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(Some(String::from_utf16_lossy(&buf[..len])))
    }

    fn write(key: &Key, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and `data` is `data.len()` bytes; both outlive the call.
        check(unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr(),
                data.len() as u32,
            )
        })
    }

    fn string_bytes(value: &str) -> Vec<u8> {
        wide(value).iter().flat_map(|c| c.to_le_bytes()).collect()
    }

    /// The user's own PATH (not the system's, which Windows adds before it).
    pub fn path() -> io::Result<String> {
        Ok(read_string(&open(ENVIRONMENT)?, "Path")?.unwrap_or_default())
    }

    /// Kept expandable, so entries written as `%USERPROFILE%\…` keep working, and announced so
    /// new terminals see it without signing out.
    pub fn set_path(value: &str) -> io::Result<()> {
        write(
            &open(ENVIRONMENT)?,
            "Path",
            REG_EXPAND_SZ,
            &string_bytes(value),
        )?;
        let environment = wide(ENVIRONMENT);
        // SAFETY: a broadcast with a NUL-terminated string that outlives the call; a window
        // that hangs is skipped after five seconds.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                std::ptr::null_mut(),
            );
        }
        Ok(())
    }

    pub fn register(
        name: &str,
        strings: &[(&str, &str)],
        numbers: &[(&str, u32)],
    ) -> io::Result<()> {
        let key = open(&format!(r"{UNINSTALL}\{name}"))?;
        for (value, text) in strings {
            write(&key, value, REG_SZ, &string_bytes(text))?;
        }
        for (value, n) in numbers {
            write(&key, value, REG_DWORD, &n.to_le_bytes())?;
        }
        Ok(())
    }

    pub fn unregister(name: &str) -> io::Result<()> {
        let sub = wide(&format!(r"{UNINSTALL}\{name}"));
        // SAFETY: `sub` is NUL-terminated and outlives the call.
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(code)
    }
}

/// Elsewhere the shell's profile owns PATH, and there is no Apps & Features.
#[cfg(not(windows))]
mod sys {
    use std::io;

    use crate::settings;

    pub const WRITES_PATH: bool = false;

    pub fn path() -> io::Result<String> {
        Ok(settings::env("PATH").unwrap_or_default())
    }

    pub fn set_path(_value: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn register(
        _name: &str,
        _strings: &[(&str, &str)],
        _numbers: &[(&str, u32)],
    ) -> io::Result<()> {
        Ok(())
    }

    pub fn unregister(_name: &str) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: Style = Style {
        sep: ';',
        ignore_case: true,
    };
    const UNIX: Style = Style {
        sep: ':',
        ignore_case: false,
    };

    #[test]
    fn a_folder_joins_the_path_once() {
        let path = r"C:\Windows;%USERPROFILE%\bin;";
        let dir = r"C:\Users\j\AppData\Local\Programs\crowbot";
        assert_eq!(
            with_dir(path, dir, WINDOWS).unwrap(),
            format!(r"C:\Windows;%USERPROFILE%\bin;{dir}")
        );
        let written = with_dir(path, dir, WINDOWS).unwrap();
        assert_eq!(with_dir(&written, &dir.to_uppercase(), WINDOWS), None);
        assert_eq!(with_dir(&format!("{written}\\"), dir, WINDOWS), None);
        assert_eq!(
            with_dir("", "/home/j/.local/bin", UNIX).unwrap(),
            "/home/j/.local/bin"
        );
        assert!(with_dir("/usr/bin:/HOME/j/.local/bin", "/home/j/.local/bin", UNIX).is_some());
    }

    #[test]
    fn a_folder_leaves_the_path_and_nothing_else_does() {
        let dir = r"C:\Users\j\AppData\Local\Programs\crowbot";
        let path = format!(r"C:\Windows;{dir}\;%USERPROFILE%\bin");
        assert_eq!(
            without_dir(&path, dir, WINDOWS).unwrap(),
            r"C:\Windows;%USERPROFILE%\bin"
        );
        assert_eq!(without_dir(r"C:\Windows", dir, WINDOWS), None);
    }
}
