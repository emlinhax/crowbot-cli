//! The card for the model's questions: a tab per question, options picked by digit or arrows
//! (several on a `multiple` one), a line for the user's own words on each, and a look at every
//! answer before they go back.

use std::collections::BTreeSet;

use crate::agent::prompt::{Answer, Question, Reply};
use crate::io::term::{KeyCode, KeyEvent};
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::tui::boxed::boxed;
use crate::tui::card::{Card, Step};
use crate::tui::editor::Editor;
use crate::tui::keymap::Action;
use crate::tui::ui;

pub struct Questions {
    id: u64,
    questions: Vec<Question>,
    drafts: Vec<Draft>,
    /// The tab in view; one past the last question is the review, when there are several.
    at: usize,
    /// Open while the user writes their own answer to the question in view.
    input: Option<Editor>,
}

#[derive(Default)]
struct Draft {
    cursor: usize,
    picked: BTreeSet<usize>,
    text: Option<String>,
    done: bool,
}

impl Questions {
    pub fn new(id: u64, questions: Vec<Question>) -> Self {
        let drafts = questions.iter().map(|_| Draft::default()).collect();
        Self {
            id,
            questions,
            drafts,
            at: 0,
            input: None,
        }
    }

    fn reviewing(&self) -> bool {
        self.at >= self.questions.len()
    }

    /// The row after a question's options is its "Other…".
    fn other(&self, q: usize) -> usize {
        self.questions[q].options.len()
    }

    /// After an answer: the next question still open, else the review (or straight back when
    /// there was only one question).
    fn advance(&mut self) -> Step {
        let count = self.questions.len();
        let open = (self.at + 1..count)
            .chain(0..self.at)
            .find(|&i| !self.drafts[i].done);
        match open {
            Some(i) => self.at = i,
            None if count == 1 => return self.send(),
            None => self.at = count,
        }
        Step::Stay
    }

    /// Every answer back to the model, once each question has one.
    fn send(&mut self) -> Step {
        if let Some(open) = self.drafts.iter().position(|d| !d.done) {
            self.at = open;
            return Step::Stay;
        }
        let answers = self
            .drafts
            .iter()
            .map(|d| Answer {
                picked: d.picked.iter().copied().collect(),
                text: d.text.clone(),
            })
            .collect();
        Step::Answer(Reply::Answers(answers))
    }

    /// Row `row` chosen on the question in view: an option is picked (or toggled on a
    /// `multiple` question), "Other…" opens a line.
    fn choose(&mut self, row: usize) -> Step {
        let q = self.at;
        self.drafts[q].cursor = row;
        if row == self.other(q) {
            let mut editor = Editor::new(0);
            if let Some(text) = &self.drafts[q].text {
                editor.set_text(text);
            }
            self.input = Some(editor);
            return Step::Stay;
        }
        let draft = &mut self.drafts[q];
        if self.questions[q].multiple {
            if !draft.picked.remove(&row) {
                draft.picked.insert(row);
            }
            return Step::Stay;
        }
        draft.picked = BTreeSet::from([row]);
        draft.text = None;
        draft.done = true;
        self.advance()
    }

    /// Enter on a `multiple` question: what is ticked is the answer, or the highlighted option
    /// when nothing is.
    fn confirm(&mut self) -> Step {
        let q = self.at;
        let cursor = self.drafts[q].cursor;
        if cursor == self.other(q) {
            return self.choose(cursor);
        }
        let draft = &mut self.drafts[q];
        if draft.picked.is_empty() && draft.text.is_none() {
            draft.picked.insert(cursor);
        }
        draft.done = true;
        self.advance()
    }

    /// The user's own words for the question in view.
    fn own_words(&mut self, words: String) -> Step {
        let q = self.at;
        let multiple = self.questions[q].multiple;
        let draft = &mut self.drafts[q];
        draft.text = Some(words);
        // A `multiple` question keeps its ticks and waits for Enter.
        if multiple {
            return Step::Stay;
        }
        draft.picked.clear();
        draft.done = true;
        self.advance()
    }

    fn last_tab(&self) -> usize {
        match self.questions.len() {
            1 => 0,
            n => n,
        }
    }

    fn tab_name(&self, q: usize) -> String {
        match &self.questions[q].header {
            Some(header) => header.clone(),
            None => fill(&ui::get().card.question_tab, &[("n", &(q + 1).to_string())]),
        }
    }

    /// What question `q` was answered with, in a line.
    fn summary(&self, q: usize) -> String {
        let text = &ui::get().card;
        let draft = &self.drafts[q];
        if !draft.done {
            return text.unanswered.clone();
        }
        let mut parts: Vec<String> = draft
            .picked
            .iter()
            .map(|&i| self.questions[q].options[i].label.clone())
            .collect();
        if let Some(own) = &draft.text {
            parts.push(fill(&text.own_words, &[("text", own)]));
        }
        parts.join(", ")
    }

    fn tabs(&self) -> Line {
        let text = &ui::get().card;
        let mut line = Line::default();
        for q in 0..self.questions.len() {
            if q > 0 {
                line.push("   ", Style::default());
            }
            let style = if q == self.at {
                Style::fg("accent").bold()
            } else {
                Style::fg("muted")
            };
            line.push(self.tab_name(q), style);
            if self.drafts[q].done {
                line.push(" ✓", Style::fg("done"));
            }
        }
        line.push("   ", Style::default());
        let style = if self.reviewing() {
            Style::fg("accent").bold()
        } else {
            Style::fg("muted")
        };
        line.push(&text.review_tab, style);
        line
    }

    fn question_lines(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().card;
        let q = self.at;
        let question = &self.questions[q];
        let draft = &self.drafts[q];
        let mut lines = Vec::new();
        let indent = Line::plain("       ");
        for row in 0..=question.options.len() {
            let chosen = row == draft.cursor;
            let (mark, style) = if chosen {
                ("›", Style::fg("accent").bold())
            } else {
                (" ", Style::default())
            };
            let other = row == question.options.len();
            let tick = match (
                question.multiple,
                draft.picked.contains(&row) || other && draft.text.is_some(),
            ) {
                (false, _) => "",
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
            };
            let label = if other {
                match &draft.text {
                    Some(own) => fill(&text.own_words, &[("text", own)]),
                    None => text.other.clone(),
                }
            } else {
                question.options[row].label.clone()
            };
            let line = Line::styled(format!(" {mark} {} {tick}{label}", row + 1), style);
            lines.push(line.truncate(width));
            if let Some(description) = question
                .options
                .get(row)
                .and_then(|o| o.description.as_ref())
            {
                let line = Line::styled(description, Style::fg("muted"));
                lines.extend(
                    line.wrap(width.saturating_sub(indent.width()), &Line::default())
                        .into_iter()
                        .map(|l| {
                            let mut out = indent.clone();
                            out.extend(l);
                            out
                        }),
                );
            }
        }
        lines
    }
}

impl Card for Questions {
    fn id(&self) -> u64 {
        self.id
    }

    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Step {
        if let Some(input) = &mut self.input {
            match action {
                Some(Action::Submit) => {
                    let words = input.text().trim().to_owned();
                    self.input = None;
                    if !words.is_empty() {
                        return self.own_words(words);
                    }
                }
                Some(Action::Escape) => self.input = None,
                Some(other) => {
                    input.apply(other);
                }
                None => input.type_key(key),
            }
            return Step::Stay;
        }
        let digit = match key.code {
            KeyCode::Char(c) => c.to_digit(10).map(|n| n as usize),
            _ => None,
        };
        match action {
            Some(Action::Escape) => return Step::Answer(Reply::No { feedback: None }),
            Some(Action::Left) => self.at = self.at.saturating_sub(1),
            Some(Action::Right | Action::Tab) => self.at = (self.at + 1).min(self.last_tab()),
            _ if self.reviewing() => match (action, digit) {
                (Some(Action::Submit), _) => return self.send(),
                (Some(Action::Up), _) => self.at = self.questions.len() - 1,
                // A digit on the review goes back to that question.
                (None, Some(n)) if (1..=self.questions.len()).contains(&n) => self.at = n - 1,
                _ => {}
            },
            Some(Action::Up) => {
                let draft = &mut self.drafts[self.at];
                draft.cursor = draft.cursor.saturating_sub(1);
            }
            Some(Action::Down) => {
                let last = self.other(self.at);
                let draft = &mut self.drafts[self.at];
                draft.cursor = (draft.cursor + 1).min(last);
            }
            Some(Action::Submit) if self.questions[self.at].multiple => return self.confirm(),
            Some(Action::Submit) => return self.choose(self.drafts[self.at].cursor),
            None if key.code == KeyCode::Char(' ') && self.questions[self.at].multiple => {
                return self.choose(self.drafts[self.at].cursor);
            }
            None => {
                if let Some(n) = digit.filter(|&n| (1..=self.other(self.at) + 1).contains(&n)) {
                    return self.choose(n - 1);
                }
            }
            _ => {}
        }
        Step::Stay
    }

    fn insert(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.insert(text);
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().card;
        let inner = width.saturating_sub(4).max(1);
        let bold = Style::default().bold();
        let mut body = Vec::new();
        if self.questions.len() > 1 {
            body.push(self.tabs());
            body.push(Line::default());
        }
        let (title, hint) = if self.reviewing() {
            for q in 0..self.questions.len() {
                let mut line = Line::styled(format!("{}  ", self.tab_name(q)), bold.clone());
                line.push(self.summary(q), Style::default());
                body.extend(line.wrap(inner, &Line::plain("  ")));
            }
            (&text.review_title, &text.review_hint)
        } else {
            let question = &self.questions[self.at];
            body.extend(
                Line::styled(&question.question, bold.clone()).wrap(inner, &Line::default()),
            );
            let hint = match (&self.input, question.multiple) {
                (Some(_), _) => &text.input_hint,
                (None, true) => &text.multiple_hint,
                (None, false) => &text.question_hint,
            };
            (&text.questions_title, hint)
        };
        let mut lines = boxed(Line::styled(title, bold), &body, width);
        if !self.reviewing() {
            lines.extend(self.question_lines(width));
            if let Some(input) = &self.input {
                let prompt = Line::styled("   ✎ ", Style::fg("accent"));
                lines.extend(input.render(width, 3, &prompt, ""));
            }
        }
        lines.push(Line::styled(format!(" {hint}"), Style::fg("muted")).truncate(width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::{KeyEventKind, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn digit(card: &mut Questions, n: char) -> Step {
        card.key(None, &press(KeyCode::Char(n)))
    }

    fn enter(card: &mut Questions) -> Step {
        card.key(Some(Action::Submit), &press(KeyCode::Enter))
    }

    fn questions(json: serde_json::Value) -> Vec<Question> {
        serde_json::from_value(json).unwrap()
    }

    fn answers(step: Step) -> Vec<Answer> {
        match step {
            Step::Answer(Reply::Answers(answers)) => answers,
            _ => panic!("the card did not answer"),
        }
    }

    #[test]
    fn one_question_answers_with_a_digit() {
        let mut card = Questions::new(
            1,
            questions(serde_json::json!([
                {"question": "Which DB?", "options": ["sqlite", "postgres"]}
            ])),
        );
        let got = answers(digit(&mut card, '2'));
        assert_eq!(
            got,
            [Answer {
                picked: vec![1],
                text: None
            }]
        );
    }

    #[test]
    fn other_keeps_a_digit_with_the_most_options_a_question_may_have() {
        let options: Vec<String> = (1..=8).map(|i| format!("option {i}")).collect();
        let mut card = Questions::new(
            1,
            questions(serde_json::json!([
                {"question": "Which?", "options": options}
            ])),
        );
        assert!(matches!(digit(&mut card, '9'), Step::Stay));
        card.insert("my own");
        let got = answers(enter(&mut card));
        assert_eq!(got[0].text.as_deref(), Some("my own"));
        assert!(got[0].picked.is_empty());
    }

    #[test]
    fn several_questions_go_back_together_after_a_review() {
        let mut card = Questions::new(
            1,
            questions(serde_json::json!([
                {"question": "Which DB?", "header": "Database", "options": ["sqlite", "postgres"]},
                {"question": "Which checks?", "header": "Checks", "options": ["lint", "test", "e2e"], "multiple": true}
            ])),
        );
        assert!(matches!(digit(&mut card, '1'), Step::Stay));
        // On the second tab now: tick two, Enter confirms them.
        let tabs = card.render(60)[1].text();
        assert!(tabs.contains("Database ✓"), "{tabs}");
        digit(&mut card, '1');
        digit(&mut card, '3');
        assert!(matches!(enter(&mut card), Step::Stay));
        let review: Vec<String> = card.render(60).iter().map(Line::text).collect();
        assert!(
            review.iter().any(|l| l.contains("Checks  lint, e2e")),
            "{review:?}"
        );
        let got = answers(enter(&mut card));
        assert_eq!(got[0].picked, [0]);
        assert_eq!(got[1].picked, [0, 2]);
    }

    #[test]
    fn the_review_will_not_send_with_a_question_unanswered() {
        let mut card = Questions::new(
            1,
            questions(serde_json::json!([
                {"question": "A?", "options": ["x"]},
                {"question": "B?", "options": ["y"]}
            ])),
        );
        card.key(Some(Action::Right), &press(KeyCode::Right));
        card.key(Some(Action::Right), &press(KeyCode::Right));
        assert!(card.reviewing());
        assert!(matches!(enter(&mut card), Step::Stay));
        assert_eq!(card.at, 0, "sent back to the first open question");
    }

    #[test]
    fn escape_dismisses_and_every_row_fits_the_width() {
        let mut card = Questions::new(
            1,
            questions(serde_json::json!([
                {"question": "A question long enough to wrap inside a narrow card?", "options": [
                    {"label": "first", "description": "what picking the first one means, at some length"}
                ]}
            ])),
        );
        assert!(card.render(30).iter().all(|l| l.width() <= 30));
        assert!(matches!(
            card.key(Some(Action::Escape), &press(KeyCode::Esc)),
            Step::Answer(Reply::No { feedback: None })
        ));
    }
}
