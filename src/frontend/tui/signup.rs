//! `/signup` inside a session: create an account in a card, show its number once, and have its
//! last digits typed back before the card closes. The proof of work is a `Job` the session loop
//! runs beside input; closing the card drops it, which stops the search.

use futures_util::future::BoxFuture;

use crate::api::error::said;
use crate::app::App;
use crate::auth::{self, SignedUp};
use crate::commands::signup::{self, confirms};
use crate::config::paths::Paths;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{self, Card, Next};
use crate::frontend::tui::choice::{Choice, Opt};
use crate::frontend::tui::editor::{Editor, Field};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::replace::{self, Asks};
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::live::prompt::Reply;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

pub struct Signup {
    stage: Stage,
    /// Where the number is saved, as the card names it.
    path: String,
}

// One card, rarely moved; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
enum Stage {
    /// Whether to replace the stored key.
    Replace(Choice),
    Creating,
    /// Whether to try again after a failure.
    Failed(Choice),
    /// The new number, saved already, and the line its last digits are typed into.
    Shown {
        number: String,
        formatted: String,
        input: Editor,
    },
}

/// Work for the session loop; what it comes to goes back through `Signup::finished`.
pub enum Job {
    Create,
}

pub enum Done {
    Created(SignedUp),
    Failed(String),
}

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        match job {
            Job::Create => match auth::sign_up(&app.api, &app.paths).await {
                Ok(account) => Done::Created(account),
                Err(e) => Done::Failed(said(&e)),
            },
        }
    })
}

impl Signup {
    /// The card, and its first work. Unless `force`, it asks first when a key is stored, as the
    /// command line refuses to replace one; `width` wraps the question.
    pub fn open(paths: &Paths, force: bool, width: usize) -> (Self, Next) {
        let path = paths.auth().display().to_string();
        let asked = match force {
            true => None,
            false => replace::question(paths, Asks::Any, width),
        };
        match asked {
            Some(choice) => (
                Self {
                    stage: Stage::Replace(choice),
                    path,
                },
                Next::Stay,
            ),
            None => (
                Self {
                    stage: Stage::Creating,
                    path,
                },
                create(),
            ),
        }
    }
}

/// That the account `number` is saved at `path`, worded by `template`.
fn saved(template: &str, number: &str, path: &str) -> String {
    fill(template, &[("hint", &auth::hint(number)), ("path", path)])
}

fn create() -> Next {
    Next::Run(card::Job::Signup(Job::Create))
}

impl Card for Signup {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        let text = &ui::get().signup;
        match &mut self.stage {
            Stage::Replace(choice) | Stage::Failed(choice) => match choice.key(action, key) {
                Next::Answer(Reply::Choice(0)) => {
                    self.stage = Stage::Creating;
                    create()
                }
                Next::Answer(_) => Next::Close(None),
                other => other,
            },
            // Only cancelling is left, and the session does that.
            Stage::Creating => Next::Stay,
            Stage::Shown { number, input, .. } => match input.field_key(action, key) {
                Field::Submit(typed) if confirms(number, &typed) => {
                    let mut done = saved(&text.done, number, &self.path);
                    done.push_str(&signup::next());
                    Next::Close(Some(done))
                }
                Field::Submit(_) => {
                    *input = Editor::new();
                    Next::Error(signup::mismatch().to_owned())
                }
                // The number is saved already; closing early only skips the check.
                Field::Cancel => Next::Close(Some(saved(&text.closed, number, &self.path))),
                Field::Edited => Next::Stay,
            },
        }
    }

    /// Typed or pasted text, for the digits line.
    fn insert(&mut self, text: &str) {
        if let Stage::Shown { input, .. } = &mut self.stage {
            input.insert(text);
        }
    }

    fn finished(&mut self, done: card::Done) -> Next {
        let card::Done::Signup(done) = done else {
            return Next::Stay;
        };
        let text = &ui::get().signup;
        match done {
            Done::Created(account) => {
                self.stage = Stage::Shown {
                    number: account.number,
                    formatted: account.formatted,
                    input: Editor::new(),
                };
                Next::Stay
            }
            Done::Failed(error) => {
                let options = vec![
                    Opt::index(text.retry.clone(), 0),
                    Opt::index(text.close.clone(), 1),
                ];
                self.stage = Stage::Failed(Choice::new(text.title.clone(), Vec::new(), options));
                Next::Error(fill(&text.failed, &[("error", &error)]))
            }
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().signup;
        let muted = Style::fg("muted");
        let (body, field) = match &self.stage {
            Stage::Replace(choice) | Stage::Failed(choice) => return choice.render(width),
            Stage::Creating => (vec![Line::styled(signup::creating(), muted)], None),
            Stage::Shown {
                formatted, input, ..
            } => {
                let mut number = Line::plain(&text.number);
                number.push(formatted, Style::fg("accent").bold());
                let mut body = vec![number, Line::default()];
                let about = Line::plain(signup::about(&self.path));
                body.extend(about.wrap(boxed::inner(width), &Line::default()));
                body.push(Line::default());
                body.push(Line::plain(signup::confirm_prompt().trim_end()));
                (body, Some(input))
            }
        };
        let mut lines = boxed(
            Line::styled(&text.title, Style::default().bold()),
            &body,
            width,
        );
        if let Some(input) = field {
            lines.extend(input.field_lines(width));
        }
        lines.push(boxed::hint(&text.hint, width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;

    fn shown(card: &Signup) -> String {
        card.render(80)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn created() -> card::Done {
        card::Done::Signup(Done::Created(SignedUp {
            number: "1234567890123456".into(),
            formatted: "1234 5678 9012 3456".into(),
        }))
    }

    fn enter() -> KeyEvent {
        press(KeyCode::Enter)
    }

    #[test]
    fn it_creates_at_once_then_wants_the_last_digits_before_it_closes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().to_path_buf());
        let (mut card, next) = Signup::open(&paths, false, 80);
        assert!(matches!(next, Next::Run(card::Job::Signup(Job::Create))));
        assert!(shown(&card).contains("proof-of-work"));

        assert!(matches!(card.finished(created()), Next::Stay));
        let text = shown(&card);
        assert!(text.contains("1234 5678 9012 3456"), "{text}");
        assert!(text.contains("no recovery"), "{text}");

        card.insert("9999");
        match card.key(Some(Action::Submit), &enter()) {
            Next::Error(e) => assert!(e.contains("does not match"), "{e}"),
            _ => panic!("expected a mismatch"),
        }
        card.insert("3456");
        match card.key(Some(Action::Submit), &enter()) {
            Next::Close(Some(done)) => assert!(done.starts_with("Account …3456 created"), "{done}"),
            _ => panic!("expected the card to close"),
        }
    }

    #[test]
    fn a_stored_key_is_asked_about_unless_forced_and_a_failure_offers_a_retry() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().to_path_buf());
        auth::save(&paths, "device-key-9999", auth::KeyKind::Device).unwrap();
        let (mut card, next) = Signup::open(&paths, false, 80);
        assert!(matches!(next, Next::Stay));
        assert!(shown(&card).contains("device key …9999"));
        assert!(matches!(
            card.key(None, &press(KeyCode::Char('2'))),
            Next::Close(None)
        ));

        let (mut card, next) = Signup::open(&paths, true, 80);
        assert!(matches!(next, Next::Run(_)));
        let failed = card::Done::Signup(Done::Failed("no network".into()));
        assert!(matches!(card.finished(failed), Next::Error(_)));
        assert!(matches!(
            card.key(None, &press(KeyCode::Char('1'))),
            Next::Run(card::Job::Signup(Job::Create))
        ));
    }
}
