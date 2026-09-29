//! The only code that touches the network, files, terminal or clock; clippy.toml bans the raw
//! calls everywhere else, so a transport change, retry or cache is one edit here.

pub mod clock;
pub mod fetch;
pub mod fs;
pub mod http;
pub mod proc;
pub mod search;
pub mod secret;
pub mod shell;
pub mod term;
