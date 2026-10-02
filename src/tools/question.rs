use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::permissions;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::agent::model_text;
use crate::agent::prompt::{self, Answer, Prompt, Reply};
use crate::limits;
use crate::permission::gate::Ask;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "question",
        include_str!("../../data/tools/question.md"),
        include_str!("../../data/tools/question.schema.json"),
    )
});

/// The card picks by digit, 1 to 9, and adds its own "Other" row after the options, so eight
/// leave every row a digit.
const MAX_OPTIONS: usize = 8;

#[derive(Deserialize)]
struct Args {
    questions: Vec<prompt::Question>,
}

pub struct Question;

impl Tool for Question {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let max = limits::get().tools.question_max.value;
        if args.questions.is_empty() || args.questions.len() > max {
            return Err(Refusal::InvalidArgs(format!(
                "ask between 1 and {max} questions"
            )));
        }
        for q in &args.questions {
            if q.question.trim().is_empty() {
                return Err(Refusal::InvalidArgs("a question has no text".into()));
            }
            if q.options.is_empty() || q.options.len() > MAX_OPTIONS {
                return Err(Refusal::InvalidArgs(format!(
                    "give each question between 1 and {MAX_OPTIONS} options"
                )));
            }
        }
        Ok(Check::new(vec![Ask::new(permissions::QUESTION.name, "*")]))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let text = model_text::get();
            let prompt = Prompt::Question {
                questions: args.questions.clone(),
            };
            match cx.ask(prompt).await {
                Some(Reply::Answers(answers)) => Output::ok(said(&args.questions, &answers)),
                Some(Reply::Unavailable) => Output::ok(text.question_unavailable.clone()),
                Some(_) => Output::ok(text.question_dismissed.clone()),
                None => Output::error(text.cancelled.clone()),
            }
        })
    }
}

/// Each question with what the user picked and said, for the model.
fn said(questions: &[prompt::Question], answers: &[Answer]) -> String {
    let text = model_text::get();
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
                parts.push(fill(&text.question_own_words, &[("text", own)]));
            }
            fill(
                &text.question_answer,
                &[("question", &q.question), ("answer", &parts.join(", "))],
            )
        })
        .collect();
    fill(&text.questions_answered, &[("answers", &lines.join("\n"))])
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
        assert_eq!(options["maxItems"], MAX_OPTIONS);
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
