//! Where the OS keeps the user's PATH and the Apps & Features entry: the registry under HKCU on
//! Windows. Elsewhere PATH belongs to the shell's profile, which crowbot only reads, and there is
//! no entry.

#[cfg(not(windows))]
pub use elsewhere::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(windows)]
mod windows {
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
    /// A window that does not take the PATH change within this long is skipped, not waited on.
    const BROADCAST_WAIT_MS: u32 = 5000;

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
    pub fn user_path() -> io::Result<String> {
        Ok(read_string(&open(ENVIRONMENT)?, "Path")?.unwrap_or_default())
    }

    /// Kept expandable, so entries written as `%USERPROFILE%\…` keep working, and announced so
    /// new terminals see it without signing out.
    pub fn set_user_path(value: &str) -> io::Result<()> {
        write(
            &open(ENVIRONMENT)?,
            "Path",
            REG_EXPAND_SZ,
            &string_bytes(value),
        )?;
        let environment = wide(ENVIRONMENT);
        // SAFETY: a broadcast with a NUL-terminated string that outlives the call.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                BROADCAST_WAIT_MS,
                std::ptr::null_mut(),
            );
        }
        Ok(())
    }

    /// The Apps & Features entry `name`: string values, then numbers.
    pub fn register_app(
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

    pub fn unregister_app(name: &str) -> io::Result<()> {
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
mod elsewhere {
    use std::io;

    use crate::io::env;

    pub const WRITES_PATH: bool = false;

    pub fn user_path() -> io::Result<String> {
        Ok(env::var("PATH").unwrap_or_default())
    }

    pub fn set_user_path(_value: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn register_app(
        _name: &str,
        _strings: &[(&str, &str)],
        _numbers: &[(&str, u32)],
    ) -> io::Result<()> {
        Ok(())
    }

    pub fn unregister_app(_name: &str) -> io::Result<()> {
        Ok(())
    }
}
