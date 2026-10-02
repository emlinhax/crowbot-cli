//! The clipboard: the system's, else the terminal's (OSC 52), which also reaches the user's own
//! machine from a session over SSH.

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
    term::to_clipboard(text);
}
