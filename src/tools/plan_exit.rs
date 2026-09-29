//! Hands PLAN mode's finished plan to the user. Always registered, so the tool list (and the
//! vendor's cache of it) does not change with the mode; outside PLAN it refuses.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::agent::model_text::{self, fill};
use crate::agent::prompt::{Prompt, Reply};
use crate::io;
use crate::mode;
use crate::permission::gate::Ask;

const CHOICES_SRC: &str = include_str!("../../data/tools/plan_exit.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "plan_exit",
        include_str!("../../data/tools/plan_exit.md"),
        include_str!("../../data/tools/plan_exit.schema.json"),
    )
});

static CHOICES: LazyLock<Vec<Choice>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        choice: Vec<Choice>,
    }
    toml::from_str::<File>(CHOICES_SRC)
        .expect("data/tools/plan_exit.toml is checked by tests")
        .choice
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    label: String,
    /// The mode to implement in; none means keep planning.
    #[serde(default)]
    mode: Option<String>,
}

const PLAN_MODE: &str = "plan";

pub struct PlanExit;

impl Tool for PlanExit {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, _args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if cx.shared.mode().id != PLAN_MODE {
            return Err(Refusal::Refused(model_text::get().plan_only.clone()));
        }
        Ok(Check::new(vec![Ask::new("plan_exit", "*")]))
    }

    fn run<'a>(&'a self, _args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let text = model_text::get();
            let plan = io::fs::read_string(std::path::Path::new(cx.plan_file))
                .ok()
                .flatten()
                .filter(|p| !p.trim().is_empty());
            let Some(plan) = plan else {
                return Output::error(fill(&text.plan_missing, &[("plan_file", cx.plan_file)]));
            };
            let prompt = Prompt::PlanExit {
                plan,
                choices: CHOICES.iter().map(|c| c.label.clone()).collect(),
            };
            match cx.ask(prompt).await {
                Some(Reply::Choice(i)) => match CHOICES.get(i).and_then(|c| c.mode.as_deref()) {
                    Some(id) => {
                        let Some(next) = mode::get(id) else {
                            return Output::error(format!("unknown mode `{id}`"));
                        };
                        cx.shared.set_mode(next);
                        Output::ok(fill(&text.plan_approved, &[("mode", &next.label)]))
                    }
                    None => Output::ok(text.plan_keep.clone()),
                },
                Some(
                    Reply::Text(feedback)
                    | Reply::No {
                        feedback: Some(feedback),
                    },
                ) => Output::ok(fill(&text.plan_changes, &[("feedback", &feedback)])),
                Some(Reply::Unavailable) => Output::ok(text.plan_unavailable.clone()),
                Some(_) => Output::ok(text.plan_keep.clone()),
                None => Output::error(text.cancelled.clone()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;
    use serde_json::json;

    #[test]
    fn choices_name_real_modes() {
        assert!(!CHOICES.is_empty());
        for choice in CHOICES.iter() {
            if let Some(id) = &choice.mode {
                assert!(mode::get(id).is_some(), "unknown mode {id}");
            }
        }
    }

    #[test]
    fn refuses_outside_plan_mode() {
        let project = Project::new();
        assert!(PlanExit.check(&json!({}), &project.cx()).is_err());
        project.shared.set_mode(mode::get("plan").unwrap());
        assert!(PlanExit.check(&json!({}), &project.cx()).is_ok());
    }
}
