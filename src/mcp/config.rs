//! A server as config.toml names it: `[mcp.<name>]` with the program, its arguments and its
//! environment. Keyed by name, so a later layer changes one field of a server or turns it off
//! (`enabled = false`).
// CEILING: local servers only, started as a program and spoken to over stdin and stdout. Upgrade:
// a `url` key for a remote server, whose HTTP task fills the same pair of line channels.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::config::limits;

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "Raw")]
pub struct Server {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "on")]
    enabled: bool,
}

fn on() -> bool {
    true
}

impl TryFrom<Raw> for Server {
    type Error = String;

    fn try_from(raw: Raw) -> Result<Self, String> {
        let command = match (raw.command, raw.enabled) {
            (Some(command), _) => command,
            (None, false) => String::new(),
            (None, true) => return Err("a server needs a `command` to run".into()),
        };
        Ok(Self {
            command,
            args: raw.args,
            env: raw.env,
            enabled: raw.enabled,
        })
    }
}

/// The `mcp` table, its names checked: each starts its tools' names, so letters, digits, `-`
/// and `_` only, no `__` (which joins it to a tool's name), and short enough to leave room.
pub fn servers<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Server>, D::Error> {
    let servers = BTreeMap::<String, Server>::deserialize(d)?;
    let most = limits::get().mcp.server_name_chars.value;
    for name in servers.keys() {
        let fits = !name.is_empty()
            && name.len() <= most
            && !name.contains("__")
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !fits {
            return Err(serde::de::Error::custom(format!(
                "`[mcp.{name}]`: a server's name is up to {most} letters, digits, `-` or `_`, \
                 without `__`"
            )));
        }
    }
    Ok(servers)
}
