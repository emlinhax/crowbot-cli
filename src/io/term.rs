#![allow(clippy::disallowed_methods)]

use std::io::{self, Write};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
pub use crossterm::event::{Event, KeyCode, KeyEventKind};

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
    use std::io::{IsTerminal, Read};
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text)?;
    Ok(Some(text).filter(|t| !t.trim().is_empty()))
}

pub fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
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
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Best effort: terminals that lack bracketed paste simply ignore the sequence.
        let _ = crossterm::execute!(stdout, EnableBracketedPaste);
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
        let mut stdout = io::stdout();
        if self.enhanced {
            let _ = crossterm::execute!(stdout, PopKeyboardEnhancementFlags);
        }
        let _ = crossterm::execute!(stdout, DisableBracketedPaste);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub fn read_event() -> io::Result<Event> {
    crossterm::event::read()
}
