//! A turn: what it is sent with (the model, the effort asked for, the system prompt written for
//! that model), and starting one with the transcript lent to it.

use super::{Tui, Turn, Turns};
use crate::agent::{RunCtx, compact, run, system_prompt};
use crate::api::models::Model;
use crate::config::paths::Paths;
use crate::conversation::message::Part;
use crate::frontend::tui::progress::Progress;
use crate::io::clock;
use crate::session::transcript::Transcript;

/// What a turn does with the transcript it is lent.
pub(super) enum Job {
    /// Answer this prompt.
    Prompt(String),
    /// Summarise earlier turns, keeping this in mind.
    Compact(Option<String>),
}

impl<'a> Tui<'a> {
    /// Starts a turn doing `job`, lending it the transcript until it hands it back.
    pub(super) fn start(&mut self, job: Job, mut owned: Transcript, turns: &Turns<'a>) -> Turn<'a> {
        let progress = Progress::start(clock::instant(), self.last_verb, &mut self.rng);
        self.last_verb = Some(progress.verb());
        self.progress = Some(progress);
        let (app, shared, shadow) = (self.app, self.shared, self.shadow);
        let (tools, emit) = (turns.tools, turns.emit);
        let next = self.next.clone();
        Box::pin(async move {
            let cx = RunCtx {
                app,
                model: &next.model,
                effort: next.asked.as_deref(),
                system: &next.system,
                tools,
                emit,
                shadow,
            };
            let ended = match job {
                Job::Prompt(text) => {
                    run::run(&cx, &mut owned, shared, vec![Part::Text { text }]).await
                }
                Job::Compact(focus) => {
                    compact::run(&cx, &mut owned, shared, focus.as_deref()).await
                }
            };
            (owned, ended)
        })
    }
}

#[derive(Clone)]
pub struct NextTurn {
    pub model: Model,
    /// The effort asked for: the setting until `/effort` changes it.
    pub asked: Option<String>,
    pub system: String,
    /// What the session's MCP servers said of their tools, kept for each prompt written.
    servers: Vec<crate::mcp::Connected>,
}

impl NextTurn {
    pub fn new(
        paths: &Paths,
        model: Model,
        asked: Option<String>,
        servers: Vec<crate::mcp::Connected>,
    ) -> Self {
        let system = system_prompt::build(paths, &model.id, &servers);
        Self {
            model,
            asked,
            system,
            servers,
        }
    }

    /// The effort the model is sent: the one asked for, when the model takes it.
    pub fn effort(&self) -> Option<&str> {
        self.model.effort(self.asked.as_deref())
    }

    /// Uses `model` from the next turn on, with a prompt written for it.
    pub fn switch(&mut self, paths: &Paths, model: Model) {
        self.system = system_prompt::build(paths, &model.id, &self.servers);
        self.model = model;
    }

    /// Writes the prompt again, since what it names (AGENTS.md, the forums) may have changed.
    pub fn reload(&mut self, paths: &Paths) {
        self.system = system_prompt::build(paths, &self.model.id, &self.servers);
    }
}
