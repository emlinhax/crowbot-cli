//! The numbered card that replaces the editor when crowbot needs an answer: permission, a
//! question, or what to do with a finished plan; `/login` builds its menu from it too.

use crate::agent::prompt::{Prompt, Reply};
use crate::auth;
use crate::io::term::{KeyCode, KeyEvent};
use crate::limits;
use crate::permission::gate;
use crate::text::diff;
use crate::text::markdown;
use crate::text::styled::{Line, Style};
use crate::tui::boxed::{boxed, capped};
use crate::tui::editor::Editor;
use crate::tui::keymap::Action;
use crate::tui::ui::{self, PermissionReply};

/// What picking an option means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Reply(PermissionReply),
    Index(usize),
    /// Opens a line for the user's own words; a masked one shows only their end.
    Text {
        masked: bool,
    },
}

pub struct Opt {
    label: String,
    pick: Pick,
}

impl Opt {
    /// Answers `Reply::Choice(index)`.
    pub fn index(label: impl Into<String>, index: usize) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Index(index),
        }
    }

    /// Opens a line whose words answer `Reply::Text`.
    pub fn text(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Text { masked: false },
        }
    }

    /// Like `text`, for a secret: only its last few characters show as it is typed.
    pub fn secret(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Text { masked: true },
        }
    }
}

pub struct Choice {
    pub id: u64,
    title: String,
    body: Vec<Line>,
    options: Vec<Opt>,
    selected: usize,
    /// Open once an option that asks for words is chosen.
    input: Option<Editor>,
    /// What the typed words answer: a reason for "No", or free text.
    input_for: Pick,
}

/// What a key did to the card.
pub enum Step {
    Stay,
    Answer(Reply),
}

impl Choice {
    pub fn new(id: u64, title: impl Into<String>, body: Vec<Line>, options: Vec<Opt>) -> Self {
        assert!(!options.is_empty(), "a card needs something to choose");
        Self {
            id,
            title: title.into(),
            body,
            options,
            selected: 0,
            input: None,
            input_for: Pick::Text { masked: false },
        }
    }

    /// The card for a prompt from the agent.
    pub fn from_prompt(id: u64, prompt: &Prompt, width: usize) -> Self {
        let text = &ui::get().card;
        let inner = width.saturating_sub(4).max(1);
        let max = limits::get().tui.prompt_body_lines.value;
        let indexed = |labels: &[String]| -> Vec<Opt> {
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| Opt::index(label.clone(), i))
                .collect()
        };
        match prompt {
            Prompt::Permission {
                tool,
                asks,
                preview,
            } => {
                // Every ask is on the card: approving one call approves all of them.
                let title = match asks.as_slice() {
                    [only] if only.permission == *tool => {
                        format!("{tool} {}", only.patterns.join(", "))
                    }
                    _ => format!("{tool}: {}", gate::describe(asks)),
                };
                let body = preview
                    .as_deref()
                    .map(|p| capped(diff::render(p, inner), max))
                    .unwrap_or_default();
                let options = text
                    .permission
                    .iter()
                    .map(|c| Opt {
                        label: c.label.clone(),
                        pick: Pick::Reply(c.reply),
                    })
                    .collect();
                Self::new(id, title, body, options)
            }
            Prompt::Question { question, options } => {
                let mut opts = indexed(options);
                opts.push(Opt::text(text.other.clone()));
                Self::new(id, question.clone(), Vec::new(), opts)
            }
            Prompt::PlanExit { plan, choices } => {
                let body = capped(markdown::render(plan, inner), max);
                let mut opts = indexed(choices);
                opts.push(Opt::text(text.plan_other.clone()));
                Self::new(id, text.plan_title.clone(), body, opts)
            }
        }
    }

    pub fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Step {
        if let Some(input) = &mut self.input {
            match action {
                Some(Action::Submit) => {
                    let words = input.text().trim().to_owned();
                    if words.is_empty() {
                        return Step::Stay;
                    }
                    return Step::Answer(match self.input_for {
                        Pick::Reply(_) => Reply::No {
                            feedback: Some(words),
                        },
                        _ => Reply::Text(words),
                    });
                }
                Some(Action::Escape) => self.input = None,
                Some(other) => {
                    input.apply(other);
                }
                None => input.type_key(key),
            }
            return Step::Stay;
        }
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => self.selected = (self.selected + 1).min(self.options.len() - 1),
            Some(Action::Submit) => return self.pick(self.selected),
            Some(Action::Escape) => return Step::Answer(Reply::No { feedback: None }),
            _ => {
                if let KeyCode::Char(c) = key.code
                    && let Some(n) = c.to_digit(10)
                    && (1..=self.options.len()).contains(&(n as usize))
                {
                    self.selected = n as usize - 1;
                    return self.pick(self.selected);
                }
            }
        }
        Step::Stay
    }

    /// Typed or pasted text, when the card has a line open for it.
    pub fn insert(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.insert(text);
        }
    }

    fn pick(&mut self, i: usize) -> Step {
        let pick = self.options[i].pick;
        match pick {
            Pick::Reply(PermissionReply::Yes) => Step::Answer(Reply::Yes),
            Pick::Reply(PermissionReply::No) => Step::Answer(Reply::No { feedback: None }),
            Pick::Index(n) => Step::Answer(Reply::Choice(n)),
            Pick::Reply(PermissionReply::NoWhy) => self.open_input(pick, false),
            Pick::Text { masked } => self.open_input(pick, masked),
        }
    }

    fn open_input(&mut self, pick: Pick, masked: bool) -> Step {
        let input = Editor::new(0);
        self.input = Some(if masked {
            input.masked(auth::HINT_CHARS)
        } else {
            input
        });
        self.input_for = pick;
        Step::Stay
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().card;
        let border = Style::fg("muted");
        let title = Line::styled(&self.title, Style::default().bold());
        let mut lines = boxed(title, &self.body, width);
        for (i, opt) in self.options.iter().enumerate() {
            let chosen = i == self.selected;
            let (mark, style) = if chosen {
                ("›", Style::fg("accent").bold())
            } else {
                (" ", Style::default())
            };
            let line = Line::styled(format!(" {mark} {} {}", i + 1, opt.label), style);
            lines.push(line.truncate(width));
        }
        let hint = match &self.input {
            Some(input) => {
                let prompt = Line::styled("   ✎ ", Style::fg("accent"));
                lines.extend(input.render(width, 3, &prompt, ""));
                &text.input_hint
            }
            None => &text.hint,
        };
        lines.push(Line::styled(format!(" {hint}"), border).truncate(width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::{KeyEventKind, KeyModifiers};
    use crate::permission::gate::Ask;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn permission() -> Choice {
        Choice::from_prompt(
            7,
            &Prompt::Permission {
                tool: "edit".into(),
                asks: vec![Ask::new("edit", "src/calc.txt")],
                preview: Some("@@ -1 +1 @@\n-2 + 2 = 5\n+2 + 2 = 4\n".into()),
            },
            50,
        )
    }

    #[test]
    fn a_permission_card_shows_the_diff_and_answers_by_number() {
        let mut card = permission();
        let text: Vec<String> = card.render(50).iter().map(Line::text).collect();
        assert!(text[0].starts_with("╭ edit src/calc.txt"), "{text:?}");
        assert!(text.iter().any(|l| l.contains("+2 + 2 = 4")));
        assert!(text.iter().all(|l| crate::text::styled::width(l) <= 50));
        assert!(matches!(
            card.key(None, &press(KeyCode::Char('1'))),
            Step::Answer(Reply::Yes)
        ));
    }

    #[test]
    fn a_call_that_asks_twice_shows_both_asks() {
        let card = Choice::from_prompt(
            8,
            &Prompt::Permission {
                tool: "read".into(),
                asks: vec![
                    Ask::new("read", "/etc/app.conf"),
                    Ask::new("external_directory", "/etc/app.conf"),
                ],
                preview: None,
            },
            100,
        );
        let top = card.render(100)[0].text();
        assert!(
            top.contains("read: read /etc/app.conf; external_directory /etc/app.conf"),
            "{top}"
        );
    }

    #[test]
    fn no_with_a_reason_opens_a_line_and_sends_it() {
        let mut card = permission();
        assert!(matches!(
            card.key(None, &press(KeyCode::Char('3'))),
            Step::Stay
        ));
        card.insert("use trash");
        match card.key(Some(Action::Submit), &press(KeyCode::Enter)) {
            Step::Answer(Reply::No { feedback }) => {
                assert_eq!(feedback.as_deref(), Some("use trash"))
            }
            _ => panic!("expected a refusal with a reason"),
        }
    }

    #[test]
    fn a_pasted_newline_in_a_reason_is_text_not_an_answer() {
        let mut card = permission();
        card.key(None, &press(KeyCode::Char('3')));
        card.insert("use trash\nnot rm");
        match card.key(Some(Action::Submit), &press(KeyCode::Enter)) {
            Step::Answer(Reply::No { feedback }) => {
                assert_eq!(feedback.as_deref(), Some("use trash\nnot rm"));
            }
            _ => panic!("expected a refusal with the whole reason"),
        }
    }

    #[test]
    fn escape_means_no_and_arrows_move() {
        let mut card = permission();
        card.key(Some(Action::Down), &press(KeyCode::Down));
        assert!(matches!(
            card.key(Some(Action::Submit), &press(KeyCode::Enter)),
            Step::Answer(Reply::No { feedback: None })
        ));
        let mut card = permission();
        assert!(matches!(
            card.key(Some(Action::Escape), &press(KeyCode::Esc)),
            Step::Answer(Reply::No { feedback: None })
        ));
    }

    #[test]
    fn questions_offer_other() {
        let mut card = Choice::from_prompt(
            1,
            &Prompt::Question {
                question: "Which DB?".into(),
                options: vec!["sqlite".into(), "postgres".into()],
            },
            40,
        );
        assert!(matches!(
            card.key(None, &press(KeyCode::Char('2'))),
            Step::Answer(Reply::Choice(1))
        ));
        card.key(None, &press(KeyCode::Char('3')));
        card.insert("duckdb");
        assert!(matches!(
            card.key(Some(Action::Submit), &press(KeyCode::Enter)),
            Step::Answer(Reply::Text(t)) if t == "duckdb"
        ));
    }
}
