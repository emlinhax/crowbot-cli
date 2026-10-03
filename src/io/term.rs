#![allow(clippy::disallowed_methods)]

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
// CEILING: key types are crossterm's; a backend swap needs a mirror type.
pub use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::{Stream, StreamExt};

use crate::settings;
use crate::text::theme::Depth;

/// Writes to stdout, ignoring a closed pipe (`crowbot models | head`) where `print!` would panic.
pub fn out(text: &str) {
    write_to(io::stdout().lock(), text);
}

pub fn err(text: &str) {
    write_to(io::stderr().lock(), text);
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

/// One line from stdin without its newline; `None` at end of input.
pub fn read_line() -> io::Result<Option<String>> {
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}

/// Reads a secret from the terminal without echoing it: Enter ends it, Esc or Ctrl+C cancels.
/// `None` when cancelled. The caller checks `stdin_is_terminal` first.
pub fn read_secret(prompt: &str) -> io::Result<Option<String>> {
    out(prompt);
    crossterm::terminal::enable_raw_mode()?;
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
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
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

type Hook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync;

/// Raw mode while the guard lives; dropping it restores the terminal, even on an early return.
/// A panic restores it too, then aborts, the same in dev as under release's `panic = "abort"`.
pub struct Raw {
    enhanced: bool,
    /// Pushed before the step it undoes, so a step that fails halfway is still undone.
    undo: Vec<Undo>,
    previous_hook: Option<Arc<Hook>>,
}

#[derive(Clone, Copy)]
enum Undo {
    #[cfg(windows)]
    CodePage(u32),
    RawMode,
    Fullscreen,
    Paste,
    Keyboard,
}

impl Undo {
    fn run(self) {
        let mut stdout = io::stdout();
        match self {
            #[cfg(windows)]
            // SAFETY: plain Win32 call with a code page the console reported.
            Undo::CodePage(page) => unsafe {
                windows_sys::Win32::System::Console::SetConsoleOutputCP(page);
            },
            Undo::RawMode => {
                let _ = crossterm::terminal::disable_raw_mode();
            }
            Undo::Fullscreen => {
                let _ = crossterm::execute!(
                    stdout,
                    EnableLineWrap,
                    DisableMouseCapture,
                    LeaveAlternateScreen
                );
            }
            Undo::Paste => {
                let _ = crossterm::execute!(stdout, DisableBracketedPaste, crossterm::cursor::Show);
            }
            Undo::Keyboard => {
                let _ = crossterm::execute!(stdout, PopKeyboardEnhancementFlags);
            }
        }
    }
}

fn undo_all(undo: &[Undo]) {
    undo.iter().rev().for_each(|u| u.run());
}

impl Raw {
    pub fn enter() -> io::Result<Self> {
        Self::open(false)
    }

    /// Raw mode on the alternate screen, with mouse reports and without auto-wrap: the whole
    /// screen is the application's until the guard drops.
    pub fn fullscreen() -> io::Result<Self> {
        Self::open(true)
    }

    fn open(fullscreen: bool) -> io::Result<Self> {
        let mut raw = Self {
            enhanced: false,
            undo: Vec::new(),
            previous_hook: None,
        };
        #[cfg(windows)]
        // SAFETY: plain Win32 calls; UTF-8 output is what every line we write is.
        unsafe {
            use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};
            raw.undo.push(Undo::CodePage(GetConsoleOutputCP()));
            SetConsoleOutputCP(65001);
        }
        crossterm::terminal::enable_raw_mode()?;
        raw.undo.push(Undo::RawMode);
        let mut stdout = io::stdout();
        // Before the keyboard flags: kitty keeps a separate flag stack per screen. Auto-wrap off:
        // a line whose width was misjudged is clipped instead of pushing rows down.
        if fullscreen {
            raw.undo.push(Undo::Fullscreen);
            crossterm::execute!(
                stdout,
                EnterAlternateScreen,
                EnableMouseCapture,
                DisableLineWrap
            )?;
        }
        raw.undo.push(Undo::Paste);
        // Best effort: terminals that lack bracketed paste simply ignore the sequence.
        let _ = crossterm::execute!(stdout, EnableBracketedPaste, crossterm::cursor::Hide);
        if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
            raw.undo.push(Undo::Keyboard);
            crossterm::execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
            raw.enhanced = true;
        }
        let previous: Arc<Hook> = Arc::from(std::panic::take_hook());
        let (undo, report) = (raw.undo.clone(), Arc::clone(&previous));
        std::panic::set_hook(Box::new(move |info| {
            undo_all(&undo);
            report(info);
            std::process::abort();
        }));
        raw.previous_hook = Some(previous);
        Ok(raw)
    }

    /// Whether the terminal speaks the kitty keyboard protocol (tells Shift+Enter from Enter).
    pub fn enhanced(&self) -> bool {
        self.enhanced
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        if let Some(previous) = self.previous_hook.take() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
        undo_all(&self.undo);
    }
}

/// What the UI reacts to.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Key(KeyEvent),
    Paste(String),
    Resize(usize, usize),
    /// Wheel notches: negative is up.
    Scroll(isize),
    /// A press on a screen row, zero-based.
    Click {
        row: usize,
        button: Button,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// Terminal input as a stream.
pub fn inputs() -> impl Stream<Item = Input> {
    EventStream::new().filter_map(|event| async move { input(event.ok()?) })
}

/// Key releases are dropped (Windows reports one for every key, which would otherwise act
/// twice), and so are mouse moves, drags and the middle button.
fn input(event: Event) -> Option<Input> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Input::Key(key)),
        Event::Paste(text) => Some(Input::Paste(text)),
        Event::Resize(w, h) => Some(Input::Resize(w as usize, h as usize)),
        Event::Mouse(mouse) => {
            let button = match mouse.kind {
                MouseEventKind::ScrollUp => return Some(Input::Scroll(-1)),
                MouseEventKind::ScrollDown => return Some(Input::Scroll(1)),
                MouseEventKind::Down(MouseButton::Left) => Button::Left,
                MouseEventKind::Down(MouseButton::Right) => Button::Right,
                _ => return None,
            };
            Some(Input::Click {
                row: mouse.row as usize,
                button,
            })
        }
        _ => None,
    }
}

/// OSC 52: asks the terminal to set the clipboard, which works across SSH.
pub fn to_clipboard(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    out(&format!("\x1b]52;c;{encoded}\x07"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::MouseEvent;

    fn mouse(kind: MouseEventKind, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn both_buttons_and_the_wheel_arrive_and_the_rest_of_the_mouse_does_not() {
        let click = |button| Some(Input::Click { row: 4, button });
        assert_eq!(
            input(mouse(MouseEventKind::Down(MouseButton::Left), 4)),
            click(Button::Left)
        );
        assert_eq!(
            input(mouse(MouseEventKind::Down(MouseButton::Right), 4)),
            click(Button::Right)
        );
        assert_eq!(
            input(mouse(MouseEventKind::ScrollUp, 0)),
            Some(Input::Scroll(-1))
        );
        assert_eq!(
            input(mouse(MouseEventKind::Down(MouseButton::Middle), 4)),
            None
        );
        assert_eq!(
            input(mouse(MouseEventKind::Drag(MouseButton::Left), 4)),
            None
        );
    }
}
