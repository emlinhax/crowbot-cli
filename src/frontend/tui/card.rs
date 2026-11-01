//! What takes the editor's place while it is up: a prompt from the agent (a permission, a
//! finished plan, the model's questions) or a session card (`/login`, `/forums`, `/models`).
//! One shape, so the session treats every card alike.

use futures_util::FutureExt;
use futures_util::future::BoxFuture;

use crate::app::App;
use crate::config::limits;
use crate::frontend::tui::boxed;
use crate::frontend::tui::choice::{Choice, Opt};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::questions::Questions;
use crate::frontend::tui::rows::capped;
use crate::frontend::tui::ui::{self, PermissionReply};
use crate::frontend::tui::{account, forums, login, signup};
use crate::io::term::KeyEvent;
use crate::live::prompt::{Prompt, Reply};
use crate::permission::gate;
use crate::text::styled::Line;
use crate::text::template::fill;
use crate::text::{diff, markdown};

/// What a card wants after a key, or after a job it asked for has finished.
pub enum Next {
    Stay,
    /// The answer to the prompt the card stands for; the card closes.
    Answer(Reply),
    /// Work for the session loop; what it comes to goes back through `Card::finished`.
    Run(Job),
    /// Say this above the card, which stays open.
    Note(String),
    /// Show an error above the card, which stays open.
    Error(String),
    /// Close the card, with what to say when there is something.
    Close(Option<String>),
    /// Use this model or preset from the next turn on; the card closes.
    UseModel(String),
    /// Write a new preset to edit; the card closes.
    NewPreset,
    /// Go on with the session in this file; the card closes.
    Resume(std::path::PathBuf),
    /// Open this file in the user's editor; the card stays open.
    Edit(std::path::PathBuf),
}

/// Network or store work a card waits on, run beside input so the screen stays live.
pub enum Job {
    Login(login::Job),
    Signup(signup::Job),
    Account(account::Job),
    /// Boxed: it carries a whole forum.
    Forums(Box<forums::Job>),
}

/// What a `Job` came to, for the card that asked for it.
pub enum Done {
    Login(login::Done),
    Signup(signup::Done),
    Account(account::Done),
    Forums(forums::Done),
}

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    match job {
        Job::Login(job) => login::run(app, job).map(Done::Login).boxed(),
        Job::Signup(job) => signup::run(app, job).map(Done::Signup).boxed(),
        Job::Account(job) => account::run(app, job).map(Done::Account).boxed(),
        Job::Forums(job) => forums::run(app, *job).map(Done::Forums).boxed(),
    }
}

pub trait Card {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next;
    /// Typed or pasted text, for a card with a line open for it.
    fn insert(&mut self, _text: &str) {}
    fn render(&self, width: usize) -> Vec<Line>;
    /// A job the card asked for has come to `done`.
    fn finished(&mut self, _done: Done) -> Next {
        Next::Stay
    }
    /// What a right-click copies while the card is up.
    fn copied(&self) -> Option<&str> {
        None
    }
}

/// The card for a prompt from the agent.
pub fn from_prompt(prompt: &Prompt, width: usize) -> Box<dyn Card> {
    match prompt {
        Prompt::Permission {
            tool,
            asks,
            preview,
            ..
        } => Box::new(permission(tool, asks, preview.as_deref(), width)),
        Prompt::PlanExit { file, choices, .. } => Box::new(plan_card(file, choices, width)),
        Prompt::Question { questions } => Box::new(Questions::new(questions.clone())),
    }
}

/// Asks to allow a call; every ask is on the card, since approving one call approves all.
fn permission(tool: &str, asks: &[gate::Ask], preview: Option<&str>, width: usize) -> Choice {
    let text = &ui::get().card;
    let inner = boxed::inner(width).max(1);
    let max = limits::get().tui.prompt_body_lines.value;
    // A call asking only under its own name reads as the ask; any other names the tool first.
    let title = match asks {
        [only] if only.permission == tool => gate::describe(asks),
        _ => format!("{tool}: {}", gate::describe(asks)),
    };
    let body = preview
        .map(|p| capped(diff::render(p, inner), max))
        .unwrap_or_default();
    let options = text
        .permission
        .iter()
        .map(|c| match c.reply {
            PermissionReply::Yes => Opt::answer(c.label.clone(), Reply::Yes),
            PermissionReply::No => Opt::answer(c.label.clone(), Reply::No { feedback: None }),
            PermissionReply::NoWhy => Opt::reason(c.label.clone()),
        })
        .collect();
    Choice::new(title, body, options)
}

/// PLAN mode's finished plan, and what to do with it.
/// The plan itself is in the transcript above the card, where it scrolls; the card says where its
/// file is.
fn plan_card(file: &str, choices: &[String], width: usize) -> Choice {
    let text = &ui::get().card;
    let inner = boxed::inner(width).max(1);
    let body = markdown::render(&fill(&text.plan_body, &[("file", file)]), inner);
    let mut options: Vec<Opt> = choices
        .iter()
        .enumerate()
        .map(|(i, label)| Opt::index(label.clone(), i))
        .collect();
    options.push(Opt::edit(text.plan_edit.clone(), file));
    options.push(Opt::text(text.plan_other.clone()));
    Choice::new(text.plan_title.clone(), body, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::permission::gate::Ask;

    fn permission() -> Choice {
        super::permission(
            "edit",
            &[Ask::new("edit", "src/calc.txt")],
            Some("@@ -1 +1 @@\n-2 + 2 = 5\n+2 + 2 = 4\n"),
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
            Next::Answer(Reply::Yes)
        ));
    }

    #[test]
    fn a_plan_card_names_the_file_and_leaves_the_plan_to_the_transcript() {
        let prompt = Prompt::PlanExit {
            plan: "1. the first step".into(),
            file: "/home/u/.crowbot/plans/s.md".into(),
            choices: vec!["Implement it".into()],
        };
        let text: Vec<String> = from_prompt(&prompt, 100)
            .render(100)
            .iter()
            .map(Line::text)
            .collect();
        assert!(
            text.iter()
                .any(|l| l.contains("/home/u/.crowbot/plans/s.md")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("Implement it")), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("first step")), "{text:?}");
    }

    #[test]
    fn a_plan_card_opens_the_plan_in_an_editor_and_stays() {
        let mut card = plan_card("/p/s.md", &["Implement it".into()], 80);
        match card.key(None, &press(KeyCode::Char('2'))) {
            Next::Edit(file) => assert_eq!(file, std::path::Path::new("/p/s.md")),
            _ => panic!("expected the plan's file to edit"),
        }
    }

    #[test]
    fn a_call_that_asks_twice_shows_both_asks() {
        let asks = [
            Ask::new("read", "/etc/app.conf"),
            Ask::new("external_directory", "/etc/app.conf"),
        ];
        let card = super::permission("read", &asks, None, 100);
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
            Next::Stay
        ));
        card.insert("use trash");
        match card.key(Some(Action::Submit), &press(KeyCode::Enter)) {
            Next::Answer(Reply::No { feedback }) => {
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
            Next::Answer(Reply::No { feedback }) => {
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
            Next::Answer(Reply::No { feedback: None })
        ));
        let mut card = permission();
        assert!(matches!(
            card.key(Some(Action::Escape), &press(KeyCode::Esc)),
            Next::Answer(Reply::No { feedback: None })
        ));
    }
}
