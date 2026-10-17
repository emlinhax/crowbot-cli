//! A server as config.toml names it: `[mcp.<name>]` with the program to start (its arguments
//! and environment) or the URL it answers at (and the headers to send), how long a call may take
//! and which of its tools to offer. Keyed by name, so a later layer changes one field of a server
//! or turns it off (`enabled = false`).

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::config::limits;

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "Raw")]
pub struct Server {
    pub reach: Reach,
    pub enabled: bool,
    /// How long a call may take, over `mcp.call_secs`.
    pub timeout_secs: Option<u64>,
    /// The tools to offer, by the server's names, `*` and `?` allowed; `None` offers them all.
    pub tools: Option<Vec<String>>,
    /// The apps listing it, when crowbot found it in their settings rather than in its own.
    pub from: Option<String>,
}

/// How crowbot reaches a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A program crowbot starts and speaks to over its stdin and stdout.
    Program {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// A server already running, spoken to over streamable HTTP.
    Url {
        url: String,
        headers: BTreeMap<String, String>,
    },
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
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default = "on")]
    enabled: bool,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    tools: Option<Vec<String>>,
}

fn on() -> bool {
    true
}

impl TryFrom<Raw> for Server {
    type Error = String;

    fn try_from(raw: Raw) -> Result<Self, String> {
        let program = |command| Reach::Program {
            command,
            args: raw.args,
            env: raw.env,
        };
        let reach = match (raw.command, raw.url) {
            (Some(_), Some(_)) => {
                return Err("a server has a `command` or a `url`, not both".into());
            }
            (Some(command), None) => program(command),
            (None, Some(url)) => Reach::Url {
                url,
                headers: raw.headers,
            },
            // Turned off, it needs neither: a later layer can switch off what an earlier named.
            (None, None) if !raw.enabled => program(String::new()),
            (None, None) => return Err("a server needs a `command` to run or a `url`".into()),
        };
        Ok(Self {
            reach,
            enabled: raw.enabled,
            timeout_secs: raw.timeout_secs,
            tools: raw.tools,
            from: None,
        })
    }
}

/// The `mcp` table, its names checked (`fits`).
pub fn servers<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Server>, D::Error> {
    let servers = BTreeMap::<String, Server>::deserialize(d)?;
    let most = limits::get().mcp.server_name_chars.value;
    for name in servers.keys() {
        if !fits(name) {
            return Err(serde::de::Error::custom(format!(
                "`[mcp.{name}]`: a server's name is up to {most} letters, digits, `-` or `_`, \
                 without `__`"
            )));
        }
    }
    Ok(servers)
}

/// Whether `name` may name a server. It starts each of its tools' names, so letters, digits, `-`
/// and `_` only, no `__` (which joins it to a tool's name), and short enough to leave room.
pub fn fits(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= limits::get().mcp.server_name_chars.value
        && !name.contains("__")
        && name.chars().all(allowed)
}

/// Another app's name for a server (`github.com/mrexodia/ida-pro-mcp`) made into one that
/// `fits`: its last `/` part, `-` for any other character, one `_` for a run of them, cut to
/// length; `None` when nothing is left.
pub fn fit(name: &str) -> Option<String> {
    let last = name.rsplit('/').next().unwrap_or(name);
    let mut out = String::new();
    for c in last.chars() {
        let c = if allowed(c) { c } else { '-' };
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
    }
    let out: String = out
        .trim_matches(['-', '_'])
        .chars()
        .take(limits::get().mcp.server_name_chars.value)
        .collect();
    let out = out.trim_end_matches(['-', '_']).to_owned();
    fits(&out).then_some(out)
}

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_apps_names_are_made_to_fit() {
        assert_eq!(
            fit("github.com/mrexodia/ida-pro-mcp").as_deref(),
            Some("ida-pro-mcp")
        );
        assert_eq!(fit("My Server (dev)").as_deref(), Some("My-Server--dev"));
        assert_eq!(fit("a__b___c").as_deref(), Some("a_b_c"));
        assert_eq!(fit("...").as_deref(), None);
        let long = fit(&"x".repeat(100)).unwrap();
        assert_eq!(long.len(), limits::get().mcp.server_name_chars.value);
        assert!(fits(&long) && !fits("a__b") && !fits("two words"));
    }
}
