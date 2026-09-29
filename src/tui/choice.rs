//! The numbered card that replaces the editor when crowbot needs an answer: permission, a
//! question, or what to do with a finished plan. Every prompt kind uses this one card.

use crate::agent::prompt::{Prompt, Reply};
use crate::io::term::{KeyCode, KeyEvent, KeyModifiers};
use crate::limits;
use crate::text::diff;
use crate::text::markdown;
use crate::text::styled::{Line, Style};
use crate::tui::editor::Editor;
use crate::tui::keymap::Action;
use crate::tui::ui::{self, PermissionReply};

/// What picking an option means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Reply(PermissionReply),
    Index(usize),
    /// Opens a line for the user's own words.
    Text,
}

struct Opt {
    label: String,
    pick: Pick,
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
    pub fn new(id: u64, prompt: &Prompt, width: usize) -> Self {
        let text = &ui::get().card;
        let inner = width.saturating_sub(4).max(1);
        let max = limits::get().tui.prompt_body_lines.value;
        let (title, body, options) = match prompt {
            Prompt::Permission {
                tool,
                asks,
                preview,
            } => {
                let target = asks
                    .first()
                    .and_then(|a| a.patterns.first())
                    .cloned()
                    .unwrap_or_default();
                let body = preview
                    .as_deref()
                    .map(|p| diff::render(p, inner, max))
                    .unwrap_or_default();
                let options = text
                    .permission
                    .iter()
                    .map(|c| Opt {
                        label: c.label.clone(),
                        pick: Pick::Reply(c.reply),
                    })
                    .collect();
                (format!("{tool} {target}"), body, options)
            }
            Prompt::Question { question, options } => {
                let mut opts: Vec<Opt> = options
                    .iter()
                    .enumerate()
                    .map(|(i, label)| Opt {
                        label: label.clone(),
                        pick: Pick::Index(i),
                    })
                    .collect();
                opts.push(Opt {
                    label: text.other.clone(),
                    pick: Pick::Text,
                });
                (question.clone(), Vec::new(), opts)
            }
            Prompt::PlanExit { plan, choices } => {
                let mut body = markdown::render(plan, inner);
                if body.len() > max {
                    let rest = body.len() - max;
                    body.truncate(max);
                    body.push(Line::styled(
                        format!("… {rest} more lines"),
                        Style::fg("muted"),
                    ));
                }
                let mut opts: Vec<Opt> = choices
                    .iter()
                    .enumerate()
                    .map(|(i, label)| Opt {
                        label: label.clone(),
                        pick: Pick::Index(i),
                    })
                    .collect();
                opts.push(Opt {
                    label: text.plan_other.clone(),
                    pick: Pick::Text,
                });
                (text.plan_title.clone(), body, opts)
            }
        };
        Self {
            id,
            title,
            body,
            options,
            selected: 0,
            input: None,
            input_for: Pick::Text,
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
                None => insert_char(input, key),
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
            Pick::Reply(PermissionReply::NoWhy) | Pick::Text => {
                self.input = Some(Editor::new(0));
                self.input_for = pick;
                Step::Stay
            }
        }
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().card;
        let border = Style::fg("muted");
        let inner = width.saturating_sub(4);
        let mut lines = Vec::new();
        let mut top = Line::styled("╭ ", border.clone());
        top.push(&self.title, Style::default().bold());
        top = top.truncate(width.saturating_sub(2));
        top.push(" ", border.clone());
        let fill = width.saturating_sub(top.width() + 1);
        top.push(format!("{}╮", "─".repeat(fill)), border.clone());
        lines.push(top);
        for body in &self.body {
            let body = body.truncate(inner);
            let mut line = Line::styled("│ ", border.clone());
            let pad = inner.saturating_sub(body.width());
            line.extend(body);
            line.push(format!("{} │", " ".repeat(pad)), border.clone());
            lines.push(line);
        }
        lines.push(Line::styled(
            format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
            border.clone(),
        ));
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

fn insert_char(input: &mut Editor, key: &KeyEvent) {
    let plain = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if let (KeyCode::Char(c), true) = (key.code, plain) {
        input.insert(&c.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::KeyEventKind;
    use crate::permission::gate::Ask;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn permission() -> Choice {
        Choice::new(
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
        let mut card = Choice::new(
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
