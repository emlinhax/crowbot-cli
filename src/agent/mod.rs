pub mod batch;
pub mod context;
pub mod doom_loop;
pub mod model_text;
pub mod run;
pub mod system_prompt;

use crate::api::models::Model;
use crate::app::App;
use crate::live::event::AgentEvent;
use crate::tools::Registry;

/// What one run works with, fixed for its length.
pub struct RunCtx<'a> {
    pub app: &'a App,
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a Registry,
    /// Where PLAN mode writes its plan; the only file it may change.
    pub plan_file: String,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}
