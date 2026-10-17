//! MCP servers the user names in config.toml (`[mcp.<name>]`): started with the session, in
//! parallel, each within `mcp.start_secs`; those that fail are said once and shown in /status.
// CEILING: tools and their instructions only; a server's resources, prompts, sampling and roots
// are not used. Upgrade: each as its own capability, behind the same `Server`.

pub mod config;
pub mod discover;
mod http;
pub mod protocol;
mod rpc;
pub mod server;

use futures_util::future::join_all;

use crate::config::paths::Paths;
use crate::config::settings::Settings;
use crate::io::http::Http;
use crate::text::template::fill;
use protocol::DATA;
use server::Server;

/// The session's servers: those running, and why the others are not.
pub struct Servers {
    running: Vec<Server>,
    failed: Vec<(String, String)>,
    off: Vec<String>,
    /// What is said of a server once, when the session starts: by its name, whole sentences.
    notes: Vec<(String, String)>,
    /// The apps listing each server found in their settings, by name.
    from: std::collections::BTreeMap<String, String>,
}

/// One server as /status shows it: its name, where it was found, and how it stands.
pub struct Standing {
    pub name: String,
    pub from: Option<String>,
    pub state: State,
}

/// How one server stands, for /status.
pub enum State {
    Running {
        tools: usize,
    },
    Failed(String),
    /// It started, then stopped.
    Stopped,
    Off,
}

impl Servers {
    /// Starts every enabled server, all at once.
    pub async fn start(settings: &Settings, paths: &Paths, web: &Http) -> Self {
        let (enabled, off): (Vec<_>, Vec<_>) =
            settings.mcp.iter().partition(|(_, cfg)| cfg.enabled);
        let started =
            join_all(enabled.iter().map(|(name, cfg)| async move {
                (name, Server::start(name, cfg, paths, web).await)
            }))
            .await;
        let mut servers = Self {
            running: Vec::new(),
            failed: Vec::new(),
            off: off.into_iter().map(|(name, _)| name.clone()).collect(),
            notes: settings.found_notes.clone(),
            from: settings
                .mcp
                .iter()
                .filter_map(|(name, cfg)| Some((name.clone(), cfg.from.clone()?)))
                .collect(),
        };
        for (name, started) in started {
            match started {
                Ok(server) => servers.running.push(server),
                Err(why) => {
                    let note = fill(&DATA.text.not_running, &[("server", name), ("why", &why)]);
                    servers.note(name, note);
                    servers.failed.push((name.to_string(), why));
                }
            }
        }
        servers
    }

    /// Those running, in name order.
    pub fn running(&self) -> &[Server] {
        &self.running
    }

    /// What is said of the servers once, when the session starts: those that did not start,
    /// and tools left out.
    pub fn notes(&self) -> &[(String, String)] {
        &self.notes
    }

    pub fn note(&mut self, server: &str, note: String) {
        self.notes.push((server.to_owned(), note));
    }

    /// Every server and how it stands, by name.
    pub fn report(&self) -> Vec<Standing> {
        let mut report: Vec<(String, State)> = self
            .running
            .iter()
            .map(|s| {
                let state = match s.stopped() {
                    true => State::Stopped,
                    false => State::Running {
                        tools: s.tools.len(),
                    },
                };
                (s.name.clone(), state)
            })
            .chain(
                self.failed
                    .iter()
                    .map(|(n, why)| (n.clone(), State::Failed(why.clone()))),
            )
            .chain(self.off.iter().map(|n| (n.clone(), State::Off)))
            .collect();
        report.sort_by(|a, b| a.0.cmp(&b.0));
        report
            .into_iter()
            .map(|(name, state)| Standing {
                from: self.from.get(&name).cloned(),
                name,
                state,
            })
            .collect()
    }

    /// What each running server said of how to use its tools, by name.
    pub fn instructions(&self) -> Vec<(String, String)> {
        self.running
            .iter()
            .filter_map(|s| Some((s.name.clone(), s.instructions.clone()?)))
            .collect()
    }

    /// Stops every server, all at once.
    pub async fn stop(self) {
        join_all(self.running.into_iter().map(Server::stop)).await;
    }
}
