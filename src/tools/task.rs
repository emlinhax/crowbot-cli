//! Hands one job to a subagent of a kind in data/agents/ and gives back its reply. The kinds and
//! what each is for fill the description and the schema's choices, so a new kind needs no edit
//! here.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::agent::child::{self, Done, Job};
use crate::agent::claims;
use crate::agent::kinds::{self, Kind};
use crate::agent::roles::Source;
use crate::data;
use crate::live::event::Outcome;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

/// The tool's name, which a subagent at the depth limit is never given.
pub const NAME: &str = "task";

/// The spec for the kinds there are now; built with the tool, since the user's agent files add
/// kinds after the built-in ones are known.
fn spec() -> Spec {
    let agents: Vec<String> = kinds::all()
        .iter()
        .map(|k| match &k.when {
            Some(when) => fill(
                &TEXT.agent_when,
                &[("id", &k.id), ("summary", &k.summary), ("when", when)],
            ),
            None => fill(&TEXT.agent, &[("id", &k.id), ("summary", &k.summary)]),
        })
        .collect();
    let description = fill(
        data::raw!("tools/task.md"),
        &[("agents", &agents.join("\n"))],
    );
    let mut spec = Spec::load(NAME, &description, data::raw!("tools/task.schema.json"));
    let ids: Vec<&str> = kinds::all().iter().map(|k| k.id.as_str()).collect();
    spec.parameters["properties"]["subagent_type"]["enum"] = json!(ids);
    spec
}

static TEXT: LazyLock<Text> = data::load!("tools/task.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    agent: String,
    agent_when: String,
    unknown_agent: String,
    no_answer: String,
    turn_limit: String,
    failed: String,
    rejected: String,
    no_run: String,
    runs_on: String,
}

#[derive(Deserialize)]
pub struct Args {
    description: String,
    prompt: String,
    subagent_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    scope: Vec<String>,
}

pub struct Task {
    spec: Spec,
}

impl Task {
    pub fn new() -> Self {
        Self { spec: spec() }
    }
}

impl Tool for Task {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &self.spec
    }

    /// The subagent stops with its parent, so its last turn and its cost are kept.
    fn stops_on_cancel(&self) -> bool {
        true
    }

    /// A model that cannot be had is refused here, before anyone is asked or anything is sent.
    fn check(&self, args: &Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let kind = kind(&args.subagent_type)?;
        let check = Check::new(vec![Ask::new(catalog::TASK.name, kind.id.clone())]);
        let Some(run) = cx.run else {
            return Ok(check);
        };
        let on = run.subagent(kind, args.model.as_deref())?;
        Ok(match on.source {
            Source::Chat => check,
            from => check.with_preview(fill(
                &TEXT.runs_on,
                &[("model", &on.model.id), ("from", &from.label(&kind.id))],
            )),
        })
    }

    fn run<'a>(&'a self, args: Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let run = cx.run.ok_or_else(|| TEXT.no_run.clone())?;
            let kind = kind(&args.subagent_type).map_err(|e| e.to_string())?;
            let on = run.subagent(kind, args.model.as_deref())?;
            let scope: Vec<String> = args.scope.iter().map(|p| claims::normalize(p)).collect();
            let job = Job {
                description: args.description.clone(),
                prompt: args.prompt,
                scope: scope.clone(),
            };
            let done = child::run_child(run, cx.call_id, kind, &on, job).await?;
            let out = said(&done).with_usage(done.usage);
            Ok(out.with_details(json!({
                "agent": kind.id,
                "model": on.model.id,
                "effort": on.effort,
                "model_from": on.source.label(&kind.id),
                "scope": scope,
                "description": args.description,
                "session": done.session.display().to_string(),
                "outcome": done.ended.outcome,
                "turns": done.ended.turns,
                "calls": done.calls,
            })))
        })
    }
}

fn kind(id: &str) -> Result<&'static Kind, Refusal> {
    kinds::get(id).ok_or_else(|| {
        let ids: Vec<String> = kinds::all().iter().map(|k| format!("`{}`", k.id)).collect();
        let why = fill(
            &TEXT.unknown_agent,
            &[("id", id), ("kinds", &ids.join(", "))],
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
    use serde_json::Value;

    use super::*;

    #[test]
    fn the_kinds_fill_the_description_and_the_choices() {
        let spec = &spec();
        assert!(
            spec.description.contains("- `explore`: "),
            "{}",
            spec.description
        );
        let choices = &spec.parameters["properties"]["subagent_type"]["enum"];
        assert_eq!(choices, &Value::from(vec!["explore", "general"]));
    }
}
