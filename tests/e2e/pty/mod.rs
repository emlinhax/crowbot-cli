//! The interactive session through a real pseudo-terminal, read back through a terminal emulator.

mod commands;
// Elsewhere a system clipboard is always there, and a test must not overwrite the developer's;
// here DISPLAY is cleared, so the copy goes out as OSC 52, which the test can read.
#[cfg(target_os = "linux")]
mod copy;
mod forums;
mod harness;
mod keytest;
mod login;
mod prompts;
mod session;
