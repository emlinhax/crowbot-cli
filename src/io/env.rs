//! The process environment, and the folders the OS names for this user.

use std::io;
use std::path::PathBuf;

// CEILING: variables that change what crowbot shows (colour depth, glyphs, clock, clipboard) are
// read across io/, and tests/e2e/main.rs `CLEARED` mirrors them by hand. A typed list here would
// let the harness clear exactly what crowbot reads.

/// One variable; empty counts as unset.
pub fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Where this user's programs keep local data (`%LOCALAPPDATA%` on Windows).
pub fn data_local_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
}

/// The directory crowbot was started in.
pub fn current_dir() -> io::Result<PathBuf> {
    std::env::current_dir()
}
