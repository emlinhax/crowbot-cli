use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::config::limits;
use crate::data;
use crate::live::prompt::{self, Answer, Prompt, Reply};
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("question");

static TEXT: LazyLock<Text> = data::load!("tools/question.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    count: String,
    no_text: String,
    options: String,
    answered: String,
    answer: String,
    own_words: String,
    dismissed: String,
    unavailable: String,
}

#[derive(Deserialize)]
pub struct Args {
    questions: Vec<prompt::Question>,
}

pub struct Question;

impl Tool for Question {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let max = limits::get().tools.question_max.value;
        if args.questions.is_empty() || args.questions.len() > max {
            return Err(Refusal::InvalidArgs(fill(
                &TEXT.count,
                &[("max", &max.to_string())],
            )));
        }
        let options_max = limits::get().tools.question_options_max.value;
        for q in &args.questions {
            if q.question.trim().is_empty() {
                return Err(Refusal::InvalidArgs(TEXT.no_text.clone()));
            }
            if q.options.is_empty() || q.options.len() > options_max {
                return Err(Refusal::InvalidArgs(fill(
                    &TEXT.options,
                    &[("max", &options_max.to_string())],
                )));
            }
        }
        Ok(Check::new(vec![Ask::new(catalog::QUESTION.name, "*")]))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let prompt = Prompt::Question {
                questions: args.questions.clone(),
            };
            Ok(match cx.ask(prompt).await {
                Some(Reply::Answers(answers)) => Output::ok(said(&args.questions, &answers)),
                Some(Reply::Unavailable) => Output::ok(TEXT.unavailable.clone()),
                Some(_) => Output::ok(TEXT.dismissed.clone()),
                None => Output::cancelled(),
            })
        })
    }
}

/// Each question with what the user picked and said, for the model.
fn said(questions: &[prompt::Question], answers: &[Answer]) -> String {
    let lines: Vec<String> = questions
        .iter()
        .zip(answers)
        .map(|(q, a)| {
            let mut parts: Vec<String> = a
                .picked
                .iter()
                .filter_map(|&i| q.options.get(i))
                .map(|o| o.label.clone())
                .collect();
            if let Some(own) = a.text.as_deref().filter(|t| !t.trim().is_empty()) {
                parts.push(fill(&TEXT.own_words, &[("text", own)]));
            }
            fill(
                &TEXT.answer,
                &[("question", &q.question), ("answer", &parts.join(", "))],
            )
        })
        .collect();
    fill(&TEXT.answered, &[("answers", &lines.join("\n"))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_offers_what_the_check_allows() {
        let questions = &SPEC.parameters["properties"]["questions"];
        assert_eq!(
            questions["maxItems"],
            limits::get().tools.question_max.value
        );
        let options = &questions["items"]["properties"]["options"];
        assert_eq!(
            options["maxItems"],
            limits::get().tools.question_options_max.value
        );
        assert_eq!(options["minItems"], 1);
    }

    #[test]
    fn the_model_hears_every_question_with_its_answer() {
        let questions: Vec<prompt::Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which database?", "options": ["SQLite", {"label": "Postgres", "description": "a server"}]},
            {"question": "Which checks?", "options": ["lint", "test", "e2e"], "multiple": true}
        ]))
        .unwrap();
        assert_eq!(questions[1].options[2].label, "e2e");
        let answers = [
            Answer {
                picked: vec![1],
                text: None,
            },
            Answer {
                picked: vec![0, 2],
                text: Some("and a smoke run".into()),
            },
        ];
        let text = said(&questions, &answers);
        assert!(text.contains("Which database?\n  Postgres"), "{text}");
        assert!(
            text.contains("lint, e2e, in their own words: and a smoke run"),
            "{text}"
        );
    }
}
