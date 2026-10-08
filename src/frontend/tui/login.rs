//! `/login` inside a session: pair this device or type an account number, in a card in place of
//! the editor. The network work is a `Job` the session loop runs beside input, so the screen
//! stays live and closing the card drops it.

use futures_util::future::BoxFuture;

use crate::api::pair::{self, Started};
use crate::app::App;
use crate::auth::{self, KeyKind};
use crate::commands::login::logged_in;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{self, Card, Next};
use crate::frontend::tui::choice::{Choice, Opt};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::live::prompt::Reply;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

pub struct Login {
    stage: Stage,
}

// One card, rarely moved; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
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

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        let done = match job {
            Job::Start => {
                return match pair::start(&app.api).await {
                    Ok(started) => Done::Started(started),
                    Err(e) => Done::Failed(format!("{e:#}")),
                };
            }
            Job::Wait(started) => auth::pair(&app.api, &app.paths, &started).await,
            Job::Check(number) => {
                auth::adopt(&app.api, &app.paths, &number, KeyKind::Account).await
            }
        };
        match done {
            Ok(done) => Done::LoggedIn(logged_in(&done)),
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
}

/// Work the card asks the session loop for.
fn work(job: Job) -> Next {
    Next::Run(card::Job::Login(job))
}

impl Card for Login {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        // Once crowbot is being asked only cancelling is left, and the session does that.
        let Stage::Choose(card) = &mut self.stage else {
            return Next::Stay;
        };
        match card.key(action, key) {
            Next::Answer(Reply::Choice(_)) => {
                self.stage = Stage::Starting;
                work(Job::Start)
            }
            Next::Answer(Reply::Text(number)) => {
                self.stage = Stage::Checking;
                work(Job::Check(number))
            }
            Next::Answer(_) => Next::Close(None),
            other => other,
        }
    }

    /// Typed or pasted text, for the account number line.
    fn insert(&mut self, text: &str) {
        if let Stage::Choose(card) = &mut self.stage {
            card.insert(text);
        }
    }

    fn finished(&mut self, done: card::Done) -> Next {
        let card::Done::Login(done) = done else {
            return Next::Stay;
        };
        match done {
            Done::Started(started) => {
                self.stage = Stage::Waiting {
                    url: pair::page(),
                    code: started.user_code.clone(),
                };
                work(Job::Wait(started))
            }
            Done::LoggedIn(message) => Next::Close(Some(message)),
            Done::Failed(error) => {
                self.stage = Stage::Choose(menu());
                Next::Error(fill(&ui::get().login.failed, &[("error", &error)]))
            }
        }
    }

    /// The pairing code while it waits to be entered.
    fn copied(&self) -> Option<&str> {
        match &self.stage {
            Stage::Waiting { code, .. } => Some(code),
            _ => None,
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
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
        lines.push(boxed::hint(&text.hint, width));
        lines
    }
}

fn menu() -> Choice {
    let text = &ui::get().login;
    let options = vec![
        Opt::index(text.pair.clone(), 0),
        Opt::secret(text.number.clone(), auth::HINT_CHARS),
    ];
    Choice::new(text.title.clone(), Vec::new(), options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;

    fn digit(c: char) -> KeyEvent {
        press(KeyCode::Char(c))
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
            login.key(None, &digit('1')),
            Next::Run(card::Job::Login(Job::Start))
        ));
        assert!(text(&login).contains("pairing code"));
        let started = Started {
            device_code: "dev1".into(),
            user_code: "ABCD-1234".into(),
            ttl: 60,
        };
        let next = login.finished(card::Done::Login(Done::Started(started)));
        assert!(matches!(next, Next::Run(card::Job::Login(Job::Wait(_)))));
        let shown = text(&login);
        assert!(shown.contains("ABCD-1234"), "{shown}");
        // While crowbot is asked the card only waits; the session closes it on Esc.
        assert!(matches!(
            login.key(Some(Action::Escape), &digit('x')),
            Next::Stay
        ));
    }

    #[test]
    fn two_takes_a_masked_number_and_a_failure_comes_back_to_the_menu() {
        let mut login = Login::new();
        login.key(None, &digit('2'));
        login.insert("1234 5678 9012 3456");
        let shown = text(&login);
        assert!(shown.contains("•••• •••• •••• 3456"), "{shown}");
        assert!(!shown.contains("1234"), "{shown}");
        match login.key(Some(Action::Submit), &press(KeyCode::Enter)) {
            Next::Run(card::Job::Login(Job::Check(number))) => {
                assert_eq!(number, "1234 5678 9012 3456")
            }
            _ => panic!("expected a check"),
        }
        match login.finished(card::Done::Login(Done::Failed("invalid_api_key".into()))) {
            Next::Error(error) => assert!(error.contains("invalid_api_key"), "{error}"),
            _ => panic!("expected the error"),
        }
        assert!(text(&login).contains("2 Enter my account number"));
    }
}
