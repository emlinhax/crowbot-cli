//! The clipboard: copying goes to the system's, else the terminal's (OSC 52), which also reaches
//! the user's own machine from a session over SSH; pasting reads the system's.

use std::sync::{LazyLock, Mutex, PoisonError};

use crate::io::term;

// On X11 the copying process serves every paste itself, so the clipboard lives as long as
// crowbot does.
// CEILING: X11 (Xwayland included) only; a Wayland session without Xwayland gets OSC 52. Upgrade:
// arboard's `wayland-data-control`, for compositors that implement it.
static SYSTEM: LazyLock<Mutex<Option<arboard::Clipboard>>> =
    LazyLock::new(|| Mutex::new(arboard::Clipboard::new().ok()));

/// Puts `text` on the clipboard.
/// CEILING: with no system clipboard and a terminal that ignores OSC 52 (GNOME Terminal over SSH),
/// nothing lands and nothing says so.
pub fn copy(text: &str) {
    let mut system = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(clipboard) = system.as_mut()
        && clipboard.set_text(text).is_ok()
    {
        return;
    }
    osc52(text);
}

/// The system clipboard's text, if it has any.
/// CEILING: the system's only, so over SSH it is the remote machine's (usually none); the
/// terminal's own paste key, which arrives as a paste, still reaches the user's clipboard.
pub fn paste() -> Option<String> {
    let mut system = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);
    let text = system.as_mut()?.get_text().ok()?;
    (!text.is_empty()).then_some(text)
}

/// Asks the terminal to set the clipboard, which works across SSH.
fn osc52(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    term::out(&format!("\x1b]52;c;{encoded}\x07"));
}
