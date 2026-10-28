//! Raw mode and the terminal state that goes with it, restored however the guard ends.

use std::io;
use std::sync::Arc;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
};

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

    /// The terminal as it was before the guard while `run` goes, for another program to use;
    /// then the guard's state again. The caller stops reading input first, or it races the
    /// program for keys.
    pub async fn lend<T>(&mut self, run: impl Future<Output = T>) -> io::Result<T> {
        let fullscreen = self.undo.iter().any(|u| matches!(u, Undo::Fullscreen));
        self.restore();
        let out = run.await;
        *self = Self::open(fullscreen)?;
        Ok(out)
    }

    fn restore(&mut self) {
        if let Some(previous) = self.previous_hook.take() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
        undo_all(&std::mem::take(&mut self.undo));
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        self.restore();
    }
}
