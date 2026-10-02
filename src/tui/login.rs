//! `/login` inside a session: pair this device or type an account number, in a card in place of
//! the editor. The network work is a `Job` the session loop runs beside input, so the screen
//! stays live and closing the card drops it.

use futures_util::future::BoxFuture;

use crate::agent::prompt::Reply;
use crate::api::pair::{self, Started};
use crate::app::App;
use crate::auth::{self, KeyKind};
use crate::commands::login::{logged_in, open_browser};
use crate::io::term::KeyEvent;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::tui::boxed::boxed;
use crate::tui::card::{Card, Step};
use crate::tui::choice::{Choice, Opt};
use crate::tui::keymap::Action;
use crate::tui::ui;

pub struct Login {
    stage: Stage,
}

enum Stage {
    Choose(Choice),
    Starting,
    Waiting { url: String, code: String },
    Checking,
}

/// Work for the session loop; what it comes to goes back through `Login::finished`.
pub enum Job {
    Start,
    Wait(Started),
    Check(String),
}

pub enum Done {
    Started(Started),
    /// The message to show.
    LoggedIn(String),
    Failed(String),
}

/// What the card wants next.
pub enum Next {
    Stay,
    Run(Job),
    /// Show an error above the card, which is back at its menu.
    Error(String),
    /// Close the card, with a message when it logged in.
    Close(Option<String>),
}

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        let (key, kind) = match job {
            Job::Start => {
                return match pair::start(&app.api).await {
                    Ok(started) => Done::Started(started),
                    Err(e) => Done::Failed(format!("{e:#}")),
                };
            }
            Job::Wait(started) => {
                open_browser(&pair::page());
                match pair::wait(&app.api, &started).await {
                    Ok(key) => (key, KeyKind::Device),
                    Err(e) => return Done::Failed(format!("{e:#}")),
                }
            }
            Job::Check(number) => (number, KeyKind::Account),
        };
        match auth::adopt(app, &key, kind).await {
            Ok(me) => Done::LoggedIn(logged_in(kind, &key, &me)),
            Err(e) => Done::Failed(format!("{e:#}")),
        }
    })
}

impl Login {
    pub fn new() -> Self {
        Self {
            stage: Stage::Choose(menu()),
        }
    }

    /// The card while a number given with `/login --key` is checked; a refusal returns to the
    /// menu, as it does for a number typed into the card.
    pub fn checking() -> Self {
        Self {
            stage: Stage::Checking,
        }
    }

    /// The pairing code while it waits to be entered, for copying.
    pub fn code(&self) -> Option<&str> {
        match &self.stage {
            Stage::Waiting { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        let Stage::Choose(card) = &mut self.stage else {
            // Only cancelling is left once crowbot is being asked.
            return match action {
                Some(Action::Escape | Action::CtrlC) => Next::Close(None),
                _ => Next::Stay,
            };
        };
        match card.key(action, key) {
            Step::Stay => Next::Stay,
            Step::Answer(Reply::Choice(_)) => {
                self.stage = Stage::Starting;
                Next::Run(Job::Start)
            }
            Step::Answer(Reply::Text(number)) => {
                self.stage = Stage::Checking;
                Next::Run(Job::Check(number))
            }
            Step::Answer(_) => Next::Close(None),
        }
    }

    /// Typed or pasted text, for the account number line.
    pub fn insert(&mut self, text: &str) {
        if let Stage::Choose(card) = &mut self.stage {
            card.insert(text);
        }
    }

    pub fn finished(&mut self, done: Done) -> Next {
        match done {
            Done::Started(started) => {
                self.stage = Stage::Waiting {
                    url: pair::page(),
                    code: started.user_code.clone(),
                };
                Next::Run(Job::Wait(started))
            }
            Done::LoggedIn(message) => Next::Close(Some(message)),
            Done::Failed(error) => {
                self.stage = Stage::Choose(menu());
                Next::Error(fill(&ui::get().login.failed, &[("error", &error)]))
            }
        }
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().login;
        let muted = Style::fg("muted");
        let body = match &self.stage {
            Stage::Choose(card) => return card.render(width),
            Stage::Starting => vec![Line::styled(&text.starting, muted.clone())],
            Stage::Checking => vec![Line::styled(&text.checking, muted.clone())],
            Stage::Waiting { url, code } => {
                let mut code_line = Line::plain(&text.code);
                code_line.push(code, Style::fg("accent").bold());
                code_line.push(&text.copy_code, muted.clone());
                vec![
                    Line::plain(fill(&text.open, &[("url", url)])),
                    code_line,
                    Line::default(),
                    Line::styled(&text.waiting, muted.clone()),
                ]
            }
        };
        let mut lines = boxed(
            Line::styled(&text.title, Style::default().bold()),
            &body,
            width,
        );
        lines.push(Line::styled(format!(" {}", text.hint), muted).truncate(width));
        lines
    }
}

fn menu() -> Choice {
    let text = &ui::get().login;
    let options = vec![
        Opt::index(text.pair.clone(), 0),
        Opt::secret(text.number.clone()),
    ];
    Choice::new(0, text.title.clone(), Vec::new(), options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::{KeyCode, KeyEventKind, KeyModifiers};

    fn press(c: char) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(c), KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn enter() -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn text(login: &Login) -> String {
        login
            .render(60)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn one_starts_pairing_and_shows_the_code() {
        let mut login = Login::new();
        assert!(text(&login).contains("1 Pair this device"));
        assert!(matches!(
            login.key(None, &press('1')),
            Next::Run(Job::Start)
        ));
        assert!(text(&login).contains("pairing code"));
        let started = Started {
            device_code: "dev1".into(),
            user_code: "ABCD-1234".into(),
            ttl: 60,
        };
        let next = login.finished(Done::Started(started));
        assert!(matches!(next, Next::Run(Job::Wait(_))));
        let shown = text(&login);
        assert!(shown.contains("ABCD-1234"), "{shown}");
        assert!(matches!(
            login.key(Some(Action::Escape), &press('x')),
            Next::Close(None)
        ));
    }

    #[test]
    fn two_takes_a_masked_number_and_a_failure_comes_back_to_the_menu() {
        let mut login = Login::new();
        login.key(None, &press('2'));
        login.insert("1234 5678 9012 3456");
        let shown = text(&login);
        assert!(shown.contains("•••• •••• •••• 3456"), "{shown}");
        assert!(!shown.contains("1234"), "{shown}");
        match login.key(Some(Action::Submit), &enter()) {
            Next::Run(Job::Check(number)) => assert_eq!(number, "1234 5678 9012 3456"),
            _ => panic!("expected a check"),
        }
        match login.finished(Done::Failed("invalid_api_key".into())) {
            Next::Error(error) => assert!(error.contains("invalid_api_key"), "{error}"),
            _ => panic!("expected the error"),
        }
        assert!(text(&login).contains("2 Enter my account number"));
    }
}
