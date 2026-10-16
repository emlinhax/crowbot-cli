//! `/account` card: where the account stands, and the keys that sign in to it, any of which but
//! this session's own can be revoked. Its jobs run beside input; closing the card drops them.

use futures_util::future::{BoxFuture, try_join};

use crate::api::account::{self, Key, Me};
use crate::api::error::said;
use crate::app::App;
use crate::commands::account::standing;
use crate::commands::login::env_note;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{self, Card, Next};
use crate::frontend::tui::choice::{Choice, Opt};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::ui::{self, AccountText};
use crate::io::term::KeyEvent;
use crate::live::prompt::Reply;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

pub struct Account {
    stage: Stage,
    me: Option<Me>,
    keys: Vec<Key>,
    /// The card's words wrap to the width it opened at.
    width: usize,
}

// One card, rarely moved; boxing the big variants would only add noise.
#[allow(clippy::large_enum_variant)]
enum Stage {
    /// Waiting on a job; what it is doing.
    Working(String),
    Overview(Choice),
    Keys(Choice),
    /// Whether to revoke `keys[index]`.
    Confirm {
        choice: Choice,
        index: usize,
    },
}

/// Work for the session loop; what it comes to goes back through `Account::finished`.
pub enum Job {
    Load,
    Revoke(Key),
}

pub enum Done {
    /// The account as it now is, and what to say first, if anything.
    Loaded {
        said: Option<String>,
        me: Me,
        keys: Vec<Key>,
    },
    Failed(String),
}

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        let text = &ui::get().account;
        let revoked = match job {
            Job::Load => None,
            Job::Revoke(key) => match account::revoke(&app.api, &key).await {
                Ok(()) => Some(named(&text.revoked, &key)),
                Err(e) => return Done::Failed(said(&e.into())),
            },
        };
        match try_join(account::me(&app.api), account::keys(&app.api)).await {
            Ok((me, keys)) => Done::Loaded {
                said: revoked,
                me,
                keys,
            },
            Err(e) => Done::Failed(said(&e.into())),
        }
    })
}

/// `template` with a key's name, hint and day filled in.
fn named(template: &str, key: &Key) -> String {
    let date = key.created_at.get(..10).unwrap_or(&key.created_at);
    fill(
        template,
        &[("name", &key.name), ("hint", &key.hint), ("date", date)],
    )
}

fn text() -> &'static AccountText {
    &ui::get().account
}

impl Account {
    /// The card, and its first work: asking crowbot where the account stands.
    pub fn open(width: usize) -> (Self, Next) {
        let card = Self {
            stage: Stage::Working(text().loading.clone()),
            me: None,
            keys: Vec::new(),
            width,
        };
        (card, Next::Run(card::Job::Account(Job::Load)))
    }

    fn overview(&self) -> Stage {
        let text = text();
        let inner = boxed::inner(self.width);
        let mut body = Vec::new();
        if let Some(me) = &self.me {
            body.extend(Line::plain(standing(me)).wrap(inner, &Line::default()));
        }
        let shadowed = env_note().trim();
        if !shadowed.is_empty() {
            body.extend(Line::styled(shadowed, Style::fg("muted")).wrap(inner, &Line::default()));
        }
        let keys = fill(&text.keys, &[("n", &self.keys.len().to_string())]);
        let options = vec![Opt::index(keys, 0), Opt::index(text.close.clone(), 1)];
        Stage::Overview(Choice::new(text.title.clone(), body, options))
    }

    fn key_list(&self) -> Stage {
        let text = text();
        let mut options: Vec<Opt> = self
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let mut label = named(&text.key, key);
                if key.current {
                    label.push_str(&text.current);
                }
                Opt::index(label, i)
            })
            .collect();
        options.push(Opt::index(text.back.clone(), self.keys.len()));
        let title = fill(&text.keys, &[("n", &self.keys.len().to_string())]);
        Stage::Keys(Choice::new(title, Vec::new(), options))
    }

    fn confirm(&self, index: usize) -> Stage {
        let text = text();
        let key = &self.keys[index];
        let warning = match key.is_account() {
            true => &text.account_warning,
            false => &text.device_warning,
        };
        let body = Line::styled(warning, Style::fg("warn"))
            .wrap(boxed::inner(self.width), &Line::default());
        let options = vec![
            Opt::index(text.revoke.clone(), 0),
            Opt::index(text.keep.clone(), 1),
        ];
        let choice = Choice::new(named(&text.confirm, key), body, options);
        Stage::Confirm { choice, index }
    }
}

impl Done {
    /// Where the account stands, when this job found out.
    pub fn me(&self) -> Option<&Me> {
        match self {
            Self::Loaded { me, .. } => Some(me),
            Self::Failed(_) => None,
        }
    }
}

impl Card for Account {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        let (stage, next) = match &mut self.stage {
            // Only cancelling is left, and the session does that.
            Stage::Working(_) => return Next::Stay,
            Stage::Overview(choice) => match choice.key(action, key) {
                Next::Answer(Reply::Choice(0)) => (self.key_list(), Next::Stay),
                Next::Answer(_) => return Next::Close(None),
                other => return other,
            },
            Stage::Keys(choice) => match choice.key(action, key) {
                Next::Answer(Reply::Choice(i)) if self.keys.get(i).is_some_and(|k| k.current) => {
                    return Next::Error(text().current_kept.clone());
                }
                Next::Answer(Reply::Choice(i)) if i < self.keys.len() => {
                    (self.confirm(i), Next::Stay)
                }
                Next::Answer(_) => (self.overview(), Next::Stay),
                other => return other,
            },
            Stage::Confirm { choice, index } => match choice.key(action, key) {
                Next::Answer(Reply::Choice(0)) => {
                    let job = Job::Revoke(self.keys[*index].clone());
                    let doing = Stage::Working(text().revoking.clone());
                    (doing, Next::Run(card::Job::Account(job)))
                }
                Next::Answer(_) => (self.key_list(), Next::Stay),
                other => return other,
            },
        };
        self.stage = stage;
        next
    }

    fn finished(&mut self, done: card::Done) -> Next {
        let card::Done::Account(done) = done else {
            return Next::Stay;
        };
        match done {
            Done::Loaded { said, me, keys } => {
                self.me = Some(me);
                self.keys = keys;
                self.stage = self.overview();
                said.map_or(Next::Stay, Next::Note)
            }
            Done::Failed(error) => {
                self.stage = self.overview();
                Next::Error(fill(&text().failed, &[("error", &error)]))
            }
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let doing = match &self.stage {
            Stage::Overview(choice) | Stage::Keys(choice) | Stage::Confirm { choice, .. } => {
                return choice.render(width);
            }
            Stage::Working(doing) => doing,
        };
        let text = text();
        let body = [Line::styled(doing, Style::fg("muted"))];
        let mut lines = boxed(
            Line::styled(&text.title, Style::default().bold()),
            &body,
            width,
        );
        lines.push(boxed::hint(&text.hint, width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;

    fn shown(card: &Account) -> String {
        card.render(80)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn digit(c: char) -> KeyEvent {
        press(KeyCode::Char(c))
    }

    fn key(name: &str, hint: &str, current: bool) -> Key {
        Key {
            reference: format!("{name:0>12}"),
            name: name.into(),
            hint: hint.into(),
            created_at: "2026-10-01T09:00:00+00:00".into(),
            current,
        }
    }

    fn loaded() -> Account {
        let (mut card, _) = Account::open(80);
        let me = Me {
            balance_microdollars: 12_300_000,
            can_spend: true,
            ..Me::default()
        };
        let keys = vec![
            key("account", "•••• 3456", false),
            key("laptop", "•••• 9999", true),
        ];
        let done = Done::Loaded {
            said: None,
            me,
            keys,
        };
        card.finished(card::Done::Account(done));
        card
    }

    #[test]
    fn the_overview_shows_the_standing_and_opens_the_keys() {
        let mut card = loaded();
        let text = shown(&card);
        assert!(text.contains("Balance $12.30."), "{text}");
        assert!(text.contains("1 Keys (2)"), "{text}");
        card.key(None, &digit('1'));
        let text = shown(&card);
        assert!(
            text.contains("account  •••• 3456  since 2026-10-01"),
            "{text}"
        );
        assert!(text.contains("laptop  •••• 9999  since 2026-10-01  · this session"));
    }

    #[test]
    fn this_sessions_key_is_never_revoked_and_another_is_after_a_warning() {
        let mut card = loaded();
        card.key(None, &digit('1'));
        assert!(matches!(card.key(None, &digit('2')), Next::Error(_)));
        card.key(None, &digit('1'));
        let text = shown(&card);
        assert!(text.contains("Revoke account •••• 3456?"), "{text}");
        assert!(text.contains("account number itself"), "{text}");
        match card.key(None, &digit('1')) {
            Next::Run(card::Job::Account(Job::Revoke(key))) => assert_eq!(key.name, "account"),
            _ => panic!("expected the revoke"),
        }
        let failed = card::Done::Account(Done::Failed("no".into()));
        assert!(matches!(card.finished(failed), Next::Error(_)));
        assert!(shown(&card).contains("Keys (2)"));
    }
}
