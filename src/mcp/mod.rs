//! MCP servers the user names in config.toml (`[mcp.<name>]`): started with the session, in
//! parallel, each within `mcp.start_secs`; those that fail are said once and shown in /status.
// CEILING: tools only; a server's resources, prompts, sampling, roots and instructions are not
// used. Upgrade: each as its own capability, behind the same `Server`.

pub mod config;
mod protocol;
mod rpc;
mod server;

use futures_util::future::join_all;

use crate::config::paths::Paths;
use crate::config::settings::Settings;
use server::Server;

/// The session's servers: those running, and why the others are not.
pub struct Servers {
    running: Vec<Server>,
    failed: Vec<(String, String)>,
    off: Vec<String>,
}

/// How one server stands, for /status.
pub enum State {
    Running { tools: usize },
    Failed(String),
    Off,
}

impl Servers {
    /// Starts every enabled server, all at once.
    pub async fn start(settings: &Settings, paths: &Paths) -> Self {
        let (enabled, off): (Vec<_>, Vec<_>) =
            settings.mcp.iter().partition(|(_, cfg)| cfg.enabled);
        let started = join_all(
            enabled
                .iter()
                .map(|(name, cfg)| async move { (name, Server::start(name, cfg, paths).await) }),
        )
        .await;
        let mut servers = Self {
            running: Vec::new(),
            failed: Vec::new(),
            off: off.into_iter().map(|(name, _)| name.clone()).collect(),
        };
        for (name, started) in started {
            match started {
                Ok(server) => servers.running.push(server),
                Err(why) => servers.failed.push((name.to_string(), why)),
            }
        }
        servers
    }

    /// The servers that did not start, each with why: said once, when the session starts.
    pub fn failures(&self) -> &[(String, String)] {
        &self.failed
    }

    /// Every server and how it stands, by name.
    pub fn report(&self) -> Vec<(String, State)> {
        let mut report: Vec<(String, State)> = self
            .running
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    State::Running {
                        tools: s.tools.len(),
                    },
                )
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
    }

    /// Stops every server, all at once.
    pub async fn stop(self) {
        join_all(self.running.into_iter().map(Server::stop)).await;
    }
}
