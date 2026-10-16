//! The agent loop: model turns, tool batches, and what the model is sent and told.

pub mod batch;
pub mod compact;
pub mod context;
pub mod doom_loop;
pub mod model_text;
pub mod overflow;
pub mod run;
pub mod setup;
pub mod system_prompt;

use std::sync::OnceLock;

use tokio_util::sync::CancellationToken;

use crate::api::models::Model;
use crate::app::App;
use crate::live::event::AgentEvent;
use crate::live::state::Shared;
use crate::snapshot::Shadow;
use crate::tools::Registry;

/// What the frontend chooses for a run, fixed for its length.
pub struct RunCtx<'a> {
    pub app: &'a App,
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a Registry,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
    /// Keeps the files as they were before the run changed them, for /undo.
    pub shadow: &'a Shadow,
}

/// One run under way: the frontend's choices, the session it drives, and what the run derives.
struct Run<'a> {
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: CancellationToken,
    /// Where PLAN mode writes its plan; the only file it may change.
    plan_file: String,
    /// The files before the run's first change, once a call that may change them is cleared.
    before: OnceLock<String>,
}
