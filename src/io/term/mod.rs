//! The terminal: what crowbot writes and reads there, and what it can show.

use std::io::{self, IsTerminal, Write};

use crossterm::event::Event;

mod caps;
mod input;
mod raw;

pub use caps::{Depth, braille, color_depth};
pub use input::{Button, Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, inputs};
pub use raw::Raw;

/// Writes to stdout, ignoring a closed pipe (`crowbot models | head`) where `print!` would panic.
pub fn out(text: &str) {
    write_to(io::stdout().lock(), text);
}

pub fn err(text: &str) {
    write_to(io::stderr().lock(), text);
}

/// One line on stderr under the program's name: `crowbot: {text}`.
pub fn warn(text: &str) {
    err(&format!("{}: {text}\n", env!("CARGO_PKG_NAME")));
}

fn write_to(mut to: impl Write, text: &str) {
    let _ = to.write_all(text.as_bytes());
    let _ = to.flush();
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

/// The width printed output fits: the terminal's, or no limit when piped, so a table piped on
/// keeps every column.
pub fn out_width() -> usize {
    match stdout_is_terminal() {
        true => size().0,
        false => usize::MAX,
    }
}

/// Columns and rows; a conventional 80×24 when the size cannot be read.
pub fn size() -> (usize, usize) {
    crossterm::terminal::size().map_or((80, 24), |(w, h)| (w as usize, h as usize))
}

/// One line from stdin without its newline; `None` at end of input.
pub fn read_line() -> io::Result<Option<String>> {
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}

/// Asks a yes/no question; an empty answer takes `default`. `None` at end of input (Ctrl+D), so
/// a closed stdin never counts as an answer.
pub fn confirm(prompt: &str, default: bool) -> io::Result<Option<bool>> {
    out(&format!(
        "{prompt} {} ",
        if default { "[Y/n]" } else { "[y/N]" }
    ));
    Ok(read_line()?.map(|line| answer(&line, default)))
}

fn answer(line: &str, default: bool) -> bool {
    match line.trim().to_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    }
}

/// Reads a secret from the terminal without echoing it: Enter ends it, Esc or Ctrl+C cancels.
/// `None` when cancelled. The caller checks `stdin_is_terminal` first.
pub fn read_secret(prompt: &str) -> io::Result<Option<String>> {
    out(prompt);
    let raw = Raw::enter()?;
    let mut secret = String::new();
    let result = loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(Some(secret)),
                KeyCode::Esc => break Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Ok(None);
                }
                KeyCode::Char(c) => secret.push(c),
                KeyCode::Backspace => {
                    secret.pop();
                }
                _ => {}
            },
            Ok(Event::Paste(text)) => {
                secret.extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')))
            }
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    drop(raw);
    out("\n");
    result
}

/// Resolves on Ctrl+C.
pub async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        // No signal handler available: never resolve rather than cancel spuriously.
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_yes_is_y_or_yes_and_an_empty_answer_takes_the_default() {
        for default in [true, false] {
            assert_eq!(answer("", default), default);
            assert_eq!(answer("  ", default), default);
        }
        for yes in ["y", "Y", "yes", " Yes "] {
            assert!(answer(yes, false), "{yes}");
        }
        for no in ["n", "no", "yep", "q"] {
            assert!(!answer(no, true), "{no}");
        }
    }
}
