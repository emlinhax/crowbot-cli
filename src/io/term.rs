#![allow(clippy::disallowed_methods)]

use std::io::{self, IsTerminal, Write};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
pub use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};

use crate::settings;
use crate::text::theme::Depth;

/// Writes to stdout, ignoring a closed pipe (`crowbot models | head`) where `print!` would panic.
pub fn out(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

pub fn err(text: &str) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// Piped input, if stdin is not a terminal; lets `git diff | crowbot -p review` work.
pub fn piped_stdin() -> io::Result<Option<String>> {
    use std::io::Read;
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text)?;
    Ok(Some(text).filter(|t| !t.trim().is_empty()))
}

pub fn stdin_is_terminal() -> bool {
    io::stdin().is_terminal()
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

/// Columns and rows; a conventional 80×24 when the size cannot be read.
pub fn size() -> (usize, usize) {
    crossterm::terminal::size().map_or((80, 24), |(w, h)| (w as usize, h as usize))
}

/// What the terminal can colour, from the conventions terminals advertise themselves by.
pub fn color_depth() -> Depth {
    if settings::env("NO_COLOR").is_some() {
        return Depth::None;
    }
    let truecolor = settings::env("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit")
        || settings::env("WT_SESSION").is_some()
        || settings::env("TERM_PROGRAM").is_some_and(|p| {
            ["iTerm.app", "WezTerm", "vscode", "ghostty", "Hyper"].contains(&p.as_str())
        })
        || settings::env("TERM").is_some_and(|t| t.contains("kitty") || t.contains("direct"));
    if truecolor {
        Depth::TrueColor
    } else {
        Depth::Ansi256
    }
}

/// Whether braille glyphs render: legacy Windows conhost has no font for them.
pub fn braille() -> bool {
    !cfg!(windows)
        || settings::env("WT_SESSION").is_some()
        || settings::env("TERM_PROGRAM").is_some()
}

/// One line from stdin without its newline; `None` at end of input.
pub fn read_line() -> io::Result<Option<String>> {
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}

/// Resolves on Ctrl+C.
pub async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        // No signal handler available: never resolve rather than cancel spuriously.
        std::future::pending::<()>().await;
    }
}

/// Raw mode while the guard lives; dropping it restores the terminal, even on an early return.
pub struct Raw {
    enhanced: bool,
}

impl Raw {
    pub fn enter() -> io::Result<Self> {
        #[cfg(windows)]
        // SAFETY: plain Win32 call; UTF-8 output is what every line we write is.
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
        }
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Best effort: terminals that lack bracketed paste simply ignore the sequence.
        let _ = crossterm::execute!(stdout, EnableBracketedPaste, crossterm::cursor::Hide);
        let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            crossterm::execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
        }
        Ok(Self { enhanced })
    }

    /// Whether the terminal speaks the kitty keyboard protocol (tells Shift+Enter from Enter).
    pub fn enhanced(&self) -> bool {
        self.enhanced
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        restore();
    }
}

/// Puts the terminal back to normal; also used by the panic hook, which has no guard to drop.
pub fn restore() {
    let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste, crossterm::cursor::Show);
    let _ = crossterm::terminal::disable_raw_mode();
}

pub fn read_event() -> io::Result<Event> {
    crossterm::event::read()
}

/// The next event if one arrives within `timeout`.
pub fn poll_event(timeout: std::time::Duration) -> io::Result<Option<Event>> {
    if crossterm::event::poll(timeout)? {
        crossterm::event::read().map(Some)
    } else {
        Ok(None)
    }
}

/// What the UI reacts to.
#[derive(Debug)]
pub enum Input {
    Key(KeyEvent),
    Paste(String),
    Resize(usize, usize),
}

/// Terminal input as a stream. Key releases are dropped: Windows reports one for every key,
/// which would otherwise act twice.
pub fn inputs() -> impl Stream<Item = Input> {
    EventStream::new().filter_map(|event| async move {
        match event.ok()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => Some(Input::Key(key)),
            Event::Paste(text) => Some(Input::Paste(text)),
            Event::Resize(w, h) => Some(Input::Resize(w as usize, h as usize)),
            _ => None,
        }
    })
}
