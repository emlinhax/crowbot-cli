//! The interactive terminal UI.

// Wired into the CLI in M3 step 3.5; until then only tests use it.
#![cfg_attr(not(test), allow(dead_code))]

pub mod editor;
pub mod input;
pub mod keymap;
pub mod screen;
