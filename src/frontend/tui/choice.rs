//! The numbered card that replaces the editor when crowbot needs an answer (permission, what to
//! do with a finished plan); the login and forums menus are built from it too.

use std::path::PathBuf;

use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::editor::{Editor, Field};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::live::prompt::Reply;
use crate::text::styled::{Line, Style};

/// What picking an option means.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    /// Answers with this reply.
    Answer(Reply),
    /// Opens a line for a reason; its words answer `No { feedback }`.
    Reason,
    /// Opens a line whose words answer `Text`; `shown` hides all but that many last characters.
    Text { shown: Option<usize> },
    /// Opens this file in the user's editor; the card stays.
    Edit(PathBuf),
}

pub struct Opt {
    label: String,
    pick: Pick,
}

impl Opt {
    pub fn answer(label: impl Into<String>, reply: Reply) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Answer(reply),
        }
    }

    /// Answers `Reply::Choice(index)`.
    pub fn index(label: impl Into<String>, index: usize) -> Self {
        Self::answer(label, Reply::Choice(index))
    }

    /// Opens a line whose words answer `Reply::Text`.
    pub fn text(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Text { shown: None },
        }
    }

    /// Like `text`, for a secret: only its last `shown` characters show as it is typed.
    pub fn secret(label: impl Into<String>, shown: usize) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Text { shown: Some(shown) },
        }
    }

    /// Opens `file` in the user's editor, and the card stays open.
    pub fn edit(label: impl Into<String>, file: impl Into<PathBuf>) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Edit(file.into()),
        }
    }

    /// Opens a line for the reason behind a "no".
    pub fn reason(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pick: Pick::Reason,
        }
    }
}

pub struct Choice {
    title: String,
    body: Vec<Line>,
    options: Vec<Opt>,
    selected: usize,
    /// Open once an option that asks for words is chosen.
    input: Option<Editor>,
    /// What the typed words answer: a reason for "No", or free text.
    input_for: Pick,
    /// The keys line under the options, when the default's "esc no" is not what Esc does.
    hint: Option<String>,
}

impl Choice {
    pub fn new(title: impl Into<String>, body: Vec<Line>, options: Vec<Opt>) -> Self {
        assert!(!options.is_empty(), "a card needs something to choose");
        Self {
            title: title.into(),
            body,
            options,
            selected: 0,
            input: None,
            input_for: Pick::Text { shown: None },
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn pick(&mut self, i: usize) -> Next {
        let pick = self.options[i].pick.clone();
        match pick {
            Pick::Answer(reply) => Next::Answer(reply),
            Pick::Reason => self.open_input(pick, None),
            Pick::Text { shown } => self.open_input(pick, shown),
            Pick::Edit(file) => Next::Edit(file),
        }
    }

    fn open_input(&mut self, pick: Pick, shown: Option<usize>) -> Next {
        let input = Editor::new();
        self.input = Some(match shown {
            Some(shown) => input.masked(shown),
            None => input,
        });
        self.input_for = pick;
        Next::Stay
    }
}

impl Card for Choice {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        if let Some(input) = &mut self.input {
            match input.field_key(action, key) {
                Field::Submit(words) => {
                    let words = words.trim().to_owned();
                    if words.is_empty() {
                        return Next::Stay;
                    }
                    return Next::Answer(match self.input_for {
                        Pick::Reason => Reply::No {
                            feedback: Some(words),
                        },
                        _ => Reply::Text(words),
                    });
                }
                Field::Cancel => self.input = None,
                Field::Edited => {}
            }
            return Next::Stay;
        }
        match action {
            Some(step @ (Action::Up | Action::Down)) => {
                self.selected = rows::step(self.selected, step, self.options.len(), false);
            }
            Some(Action::Submit) => return self.pick(self.selected),
            Some(Action::Escape) => return Next::Answer(Reply::No { feedback: None }),
            _ => {
                if let Some(i) = rows::digit(key, self.options.len()) {
                    self.selected = i;
                    return self.pick(i);
                }
            }
        }
        Next::Stay
    }

    /// Typed or pasted text, when the card has a line open for it.
    fn insert(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.insert(text);
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().card;
        let title = Line::styled(&self.title, Style::default().bold());
        let mut lines = boxed(title, &self.body, width);
        for (i, opt) in self.options.iter().enumerate() {
            let line = rows::option(i + 1, &opt.label, "", i == self.selected);
            lines.push(line.truncate(width));
        }
        let hint = match &self.input {
            Some(input) => {
                lines.extend(input.field_lines(width));
                &text.input_hint
            }
            None => self.hint.as_ref().unwrap_or(&text.hint),
        };
        lines.push(boxed::hint(hint, width));
        lines
    }
}
