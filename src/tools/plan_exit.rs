//! Hands PLAN mode's finished plan to the user. Always registered, so the tool list (and the
//! vendor's cache of it) does not change with the mode; outside PLAN it refuses.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde::de::IgnoredAny;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::data;
use crate::io;
use crate::live::prompt::{Prompt, Reply};
use crate::mode;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("plan_exit");

static TEXT: LazyLock<Text> = data::load!("tools/plan_exit.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    only: String,
    missing: String,
    approved: String,
    keep: String,
    changes: String,
    unavailable: String,
    unknown_mode: String,
    choice: Vec<Choice>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    label: String,
    /// The mode to implement in; none means keep planning.
    #[serde(default)]
    mode: Option<String>,
}

pub struct PlanExit;

impl Tool for PlanExit {
    type Args = IgnoredAny;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, _args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if !cx.shared.mode().plan_exit {
            return Err(TEXT.only.clone().into());
        }
        Ok(Check::new(vec![Ask::new(catalog::PLAN_EXIT.name, "*")]))
    }

    fn run<'a>(
        &'a self,
        _args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let plan = io::fs::read_string(std::path::Path::new(cx.plan_file))
                .ok()
                .flatten()
                .filter(|p| !p.trim().is_empty());
            let Some(plan) = plan else {
                return Err(fill(&TEXT.missing, &[("plan_file", cx.plan_file)]));
            };
            let prompt = Prompt::PlanExit {
                plan,
                choices: TEXT.choice.iter().map(|c| c.label.clone()).collect(),
            };
            Ok(match cx.ask(prompt).await {
                Some(Reply::Choice(i)) => {
                    match TEXT.choice.get(i).and_then(|c| c.mode.as_deref()) {
                        Some(id) => {
                            let Some(next) = mode::get(id) else {
                                return Err(fill(&TEXT.unknown_mode, &[("mode", id)]));
                            };
                            cx.shared.set_mode(next);
                            Output::ok(fill(&TEXT.approved, &[("mode", &next.label)]))
                        }
                        None => Output::ok(TEXT.keep.clone()),
                    }
                }
                Some(
                    Reply::Text(feedback)
                    | Reply::No {
                        feedback: Some(feedback),
                    },
                ) => Output::ok(fill(&TEXT.changes, &[("feedback", &feedback)])),
                Some(Reply::Unavailable) => Output::ok(TEXT.unavailable.clone()),
                Some(_) => Output::ok(TEXT.keep.clone()),
                None => Output::cancelled(),
            })
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
        assert!(!TEXT.choice.is_empty());
        for choice in TEXT.choice.iter() {
            if let Some(id) = &choice.mode {
                assert!(mode::get(id).is_some(), "unknown mode {id}");
            }
        }
    }

    #[test]
    fn refuses_outside_plan_mode() {
        let project = Project::new();
        assert!(project.check(&PlanExit, &json!({})).is_err());
        project.shared.set_mode(mode::get("plan").unwrap());
        assert!(project.check(&PlanExit, &json!({})).is_ok());
    }
}
