//! Hands one job to a subagent of a role in data/agents/ (and ~/.crowbot/agents/) and gives back
//! its reply. The roles themselves are named in the system prompt, which is written again when one
//! is added; this tool only says what a role is for, so a new role needs no edit here.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::agent::child::{self, Done};
use crate::agent::roles::{self, Role};
use crate::data;
use crate::live::event::Outcome;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

/// The tool's name, which a subagent at the depth limit is never given.
pub const NAME: &str = "task";

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        NAME,
        data::raw!("tools/task.md"),
        data::raw!("tools/task.schema.json"),
    )
});

static TEXT: LazyLock<Text> = data::load!("tools/task.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    unknown_agent: String,
    no_answer: String,
    turn_limit: String,
    failed: String,
    rejected: String,
    no_run: String,
}

#[derive(Deserialize)]
pub struct Args {
    description: String,
    prompt: String,
    subagent_type: String,
}

pub struct Task;

impl Tool for Task {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    /// The subagent stops with its parent, so its last turn and its cost are kept.
    fn stops_on_cancel(&self) -> bool {
        true
    }

    fn check(&self, args: &Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let role = role(&args.subagent_type, cx)?;
        Ok(Check::new(vec![Ask::new(
            catalog::TASK.name,
            role.id.clone(),
        )]))
    }

    fn run<'a>(&'a self, args: Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let run = cx.run.ok_or_else(|| TEXT.no_run.clone())?;
            let role = role(&args.subagent_type, cx).map_err(|e| e.to_string())?;
            let done = child::run_child(run, cx.call_id, &role, args.prompt).await;
            let out = said(&done).with_usage(done.usage);
            Ok(out.with_details(json!({
                "agent": role.id,
                "description": args.description,
                "session": done.session.display().to_string(),
                "outcome": done.ended.outcome,
                "turns": done.ended.turns,
                "calls": done.calls,
            })))
        })
    }
}

/// The role `id` names as the machine has it now: a role the user added mid-session dispatches
/// without a restart, and a machine with no roles at all has the fallback one.
fn role(id: &str, cx: &ToolCx<'_>) -> Result<Role, Refusal> {
    let roles = roles::dispatchable(&cx.app.paths);
    roles
        .iter()
        .find(|role| role.id == id)
        .cloned()
        .ok_or_else(|| {
            let ids: Vec<String> = roles.iter().map(|r| format!("`{}`", r.id)).collect();
            let why = fill(
                &TEXT.unknown_agent,
                &[("id", id), ("roles", &ids.join(", "))],
            );
            Refusal::Refused(why)
        })
}

/// What the model is told of the subagent's run: its reply, or why there is none.
fn said(done: &Done) -> Output {
    let answer = done.answer.clone();
    match done.ended.outcome {
        Outcome::Done => Output::ok(answer.unwrap_or_else(|| TEXT.no_answer.clone())),
        Outcome::TurnLimit => {
            let turns = done.ended.turns.to_string();
            let answer = answer.unwrap_or_default();
            Output::error(fill(
                &TEXT.turn_limit,
                &[("turns", &turns), ("answer", &answer)],
            ))
        }
        Outcome::Failed => {
            let error = done.error.clone().unwrap_or_default();
            Output::error(fill(&TEXT.failed, &[("error", &error)]))
        }
        // A plain No to the subagent is a No to the run that started it.
        Outcome::Rejected => Output::error(TEXT.rejected.clone()).stopping(),
        Outcome::Aborted => Output::cancelled(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_this_machine_does_not_have_is_refused_by_name() {
        let project = crate::tools::testing::Project::new();
        crate::config::prefs::set_enabled(&project.app.paths, "worker", true);
        let cx = project.cx();
        let why = role("nobody", &cx).unwrap_err().to_string();
        assert!(why.contains("no role called `nobody`"), "{why}");
        assert!(why.contains("`worker`"), "{why}");
        assert_eq!(role("worker", &cx).unwrap().id, "worker");
        // A role that is off is not one this machine dispatches either.
        let why = role("reader", &cx).unwrap_err().to_string();
        assert!(why.contains("no role called `reader`"), "{why}");
    }

    #[test]
    fn the_schema_leaves_the_role_open_since_the_prompt_names_them() {
        assert!(SPEC.parameters["properties"]["subagent_type"]["enum"].is_null());
    }
}
