//! The only code that touches the network, files, processes, terminal or clock; clippy.toml bans
//! the raw calls elsewhere.
#![allow(clippy::disallowed_methods)]

pub mod clipboard;
pub mod clock;
pub mod env;
pub mod exe;
pub mod fetch;
pub mod fs;
pub mod http;
pub mod proc;
pub mod registry;
pub mod search;
pub mod secret;
pub mod term;
#[cfg(windows)]
mod win;
