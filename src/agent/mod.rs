pub mod batch;
pub mod context;
pub mod doom_loop;
pub mod model_text;
pub mod run;
pub mod setup;
pub mod system_prompt;

use tokio_util::sync::CancellationToken;

use crate::api::models::Model;
use crate::app::App;
use crate::live::event::AgentEvent;
use crate::live::state::Shared;
use crate::tools::Registry;

/// What the frontend chooses for a run, fixed for its length.
pub struct RunCtx<'a> {
    pub app: &'a App,
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a Registry,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

/// One run under way: the frontend's choices, the session it drives, and what the run derives.
struct Run<'a> {
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: CancellationToken,
    /// Where PLAN mode writes its plan; the only file it may change.
    plan_file: String,
}
