use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::agent::model_text::{self, fill};
use crate::agent::prompt::{Prompt, Reply};
use crate::permission::gate::Ask;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "question",
        include_str!("../../data/tools/question.md"),
        include_str!("../../data/tools/question.schema.json"),
    )
});

/// Options are picked by digit, so more than nine cannot all be chosen with one key.
const MAX_OPTIONS: usize = 9;

#[derive(Deserialize)]
struct Args {
    question: String,
    options: Vec<String>,
}

pub struct Question;

impl Tool for Question {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        if args.options.is_empty() || args.options.len() > MAX_OPTIONS {
            return Err(Refusal::InvalidArgs(format!(
                "give between 1 and {MAX_OPTIONS} options"
            )));
        }
        Ok(Check::new(vec![Ask::new("question", "*")]))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let text = model_text::get();
            let prompt = Prompt::Question {
                question: args.question,
                options: args.options.clone(),
            };
            match cx.ask(prompt).await {
                Some(Reply::Choice(i)) if i < args.options.len() => {
                    Output::ok(fill(&text.question_chosen, &[("answer", &args.options[i])]))
                }
                Some(
                    Reply::Text(answer)
                    | Reply::No {
                        feedback: Some(answer),
                    },
                ) => Output::ok(fill(&text.question_answered, &[("answer", &answer)])),
                Some(Reply::Unavailable) => Output::ok(text.question_unavailable.clone()),
                Some(_) => Output::ok(text.question_dismissed.clone()),
                None => Output::error(text.cancelled.clone()),
            }
        })
    }
}
