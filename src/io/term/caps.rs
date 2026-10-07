//! What the terminal can show: colour depth and braille.

use crate::io::env;

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    /// `NO_COLOR`: attributes only.
    None,
}

/// What the terminal can colour, from the conventions terminals advertise themselves by.
pub fn color_depth() -> Depth {
    if env::var("NO_COLOR").is_some() {
        return Depth::None;
    }
    let truecolor = env::var("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit")
        || env::var("WT_SESSION").is_some()
        || env::var("TERM_PROGRAM").is_some_and(|p| {
            ["iTerm.app", "WezTerm", "vscode", "ghostty", "Hyper"].contains(&p.as_str())
        })
        || env::var("TERM").is_some_and(|t| t.contains("kitty") || t.contains("direct"));
    if truecolor {
        Depth::TrueColor
    } else {
        Depth::Ansi256
    }
}

/// Whether braille glyphs render: legacy Windows conhost has no font for them.
pub fn braille() -> bool {
    !cfg!(windows)
        || env::var("WT_SESSION").is_some()
        || env::var("TERM_PROGRAM").is_some()
        || hosted_console()
}

/// A program a terminal hosts (Windows Terminal opening a double-clicked .exe, which sets no
/// variable) owns a hidden pseudo-console window; legacy conhost draws its own window instead.
#[cfg(windows)]
fn hosted_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut class = [0u16; 64];
    // SAFETY: plain Win32 calls; the buffer outlives the call, which is told its length.
    let len = unsafe {
        let window = GetConsoleWindow();
        if window.is_null() {
            return false;
        }
        GetClassNameW(window, class.as_mut_ptr(), class.len() as i32)
    };
    let len = usize::try_from(len).unwrap_or(0);
    String::from_utf16_lossy(&class[..len]) == "PseudoConsoleWindow"
}

#[cfg(not(windows))]
fn hosted_console() -> bool {
    false
}
