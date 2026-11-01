//! The agent loop: model turns, tool batches, and what the model is sent and told.

pub mod batch;
pub mod child;
pub mod compact;
pub mod context;
pub mod doom_loop;
pub mod kinds;
pub mod model_text;
pub mod orient;
pub mod overflow;
pub mod run;
pub mod setup;
pub mod system_prompt;
#[cfg(test)]
pub mod testing;
pub mod verify;
pub mod written;

use tokio_util::sync::CancellationToken;

use crate::api::models::Model;
use crate::app::App;
use crate::live::event::AgentEvent;
use crate::live::state::Shared;
use crate::models::preset::{Routes, Signals};
use crate::session::transcript::Transcript;
use crate::snapshot::Shadow;
use crate::tools::Registry;

/// What the frontend chooses for a run, fixed for its length.
#[derive(Clone, Copy)]
pub struct RunCtx<'a> {
    pub app: &'a App,
    /// Where each request goes, unless `preset` picks another.
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    /// Picks each request's model and effort, when the session runs on a preset.
    pub preset: Option<&'a Routes>,
    /// The kind of subagent this run is; `None` for the run a frontend started.
    pub agent: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a Registry,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
    /// Keeps the files as they were before the run changed them, for /undo.
    pub shadow: &'a Shadow,
}

impl<'a> RunCtx<'a> {
    /// The next request's model and effort: the preset's pick from where the run stands now,
    /// else the session's.
    pub fn picked(&self, transcript: &Transcript, shared: &Shared) -> Self {
        let Some(preset) = self.preset else {
            return *self;
        };
        let account = shared.account();
        let signals = Signals {
            mode: &shared.mode().id,
            agent: self.agent,
            context: transcript.last_used().map(|(_, usage)| usage.input),
            account: account.as_ref(),
        };
        let (model, effort) = preset.pick(&signals);
        Self {
            model,
            effort: effort.or(self.effort),
            ..*self
        }
    }

    /// What the session chose, as a prompt records it to go on with: the preset, else the model.
    pub fn chosen(&self) -> &str {
        self.preset.map_or(&self.model.id, |p| &p.id)
    }
}

/// One run under way: the frontend's choices, the session it drives, and what the run derives.
pub(crate) struct Run<'a> {
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: CancellationToken,
    /// Where PLAN mode writes its plan; the only file it may change. A subagent's is its
    /// parent's.
    plan_file: String,
    /// The files before the prompt's first change, once a call that may change them is
    /// cleared; a subagent's change takes its parent's (`root`), so one prompt has one.
    before: tokio::sync::OnceCell<String>,
    /// The session file the run writes, which a subagent's file is kept beside.
    session: std::path::PathBuf,
    /// The run whose `task` call started this one; `None` for the run a frontend started.
    parent: Option<&'a Run<'a>>,
    /// Places for the subagents this run starts to work at once (`subagent.parallel`).
    // CEILING: the places are per run, so a subagent waiting on its own subagents holds one
    // meanwhile; moot while `subagent.max_depth` is 1. Upgrade: one pool for the whole tree.
    slots: tokio::sync::Semaphore,
}

impl Run<'_> {
    /// How many runs started this one: 0 for a frontend's.
    pub(crate) fn depth(&self) -> usize {
        self.parent.map_or(0, |p| p.depth() + 1)
    }

    /// The run a frontend started, which this one is under.
    pub(crate) fn root(&self) -> &Run<'_> {
        self.parent.map_or(self, |p| p.root())
    }
}
