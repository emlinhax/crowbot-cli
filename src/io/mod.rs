//! The only code that touches the network, files, terminal or clock; clippy.toml bans the raw
//! calls everywhere else, so a transport change, retry or cache is one edit here.
#![allow(clippy::disallowed_methods)]

pub mod clipboard;
pub mod clock;
pub mod env;
pub mod exe;
pub mod fetch;
pub mod fs;
pub mod http;
pub mod proc;
pub mod search;
pub mod secret;
pub mod term;
pub mod user_path;
