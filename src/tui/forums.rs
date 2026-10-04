//! `/forums` inside a session: the forums the agent can browse, as a list in a card in place of
//! the editor. A forum opens a menu to log in (a username, then a hidden password), log out or
//! remove it; the last row adds one from Tapatalk's directory. Network and store work is a `Job`
//! the session loop runs beside input, as the login card's is, so closing the card drops it.

use futures_util::future::BoxFuture;

use crate::agent::prompt::Reply;
use crate::app::App;
use crate::commands::Scope;
use crate::commands::forums::{forget, keep, sign_in, sign_out};
use crate::forums::{self, Forum, directory, store};
use crate::io::term::KeyEvent;
use crate::limits;
use crate::text::styled::{Line, Style};
use crate::text::table::{self, Align};
use crate::text::template::fill;
use crate::tui::boxed::boxed;
use crate::tui::card::{Card, Step};
use crate::tui::choice::{Choice, Opt};
use crate::tui::editor::Editor;
use crate::tui::keymap::Action;
use crate::tui::picker::{self, GAP, MARKER};
use crate::tui::ui;

pub struct Forums {
    forums: Vec<Forum>,
    /// The highlighted row of the list; one past the last forum is the row that adds one.
    selected: usize,
    stage: Stage,
}

enum Stage {
    List,
    /// What can be done with the highlighted forum, in its menu's order.
    Menu {
        choice: Choice,
        acts: Vec<Act>,
    },
    /// Suggested searches from `data/forums.toml`, then a line for the user's own words.
    Add {
        choice: Choice,
        queries: Vec<String>,
    },
    Found {
        query: String,
        results: Vec<Forum>,
        selected: usize,
    },
    Username(Editor),
    Password {
        user: String,
        input: Editor,
    },
    /// Waiting on a job: what it is doing, and where a failure goes back to.
    Working {
        doing: String,
        back: Back,
    },
}

#[derive(Clone, Copy)]
enum Act {
    LogIn,
    LogOut,
    Remove,
}

enum Back {
    List,
    Add,
    /// The username step, with the name already typed.
    Username(String),
}

/// Work for the session loop; what it comes to goes back through `Forums::finished`.
pub enum Job {
    Search(String),
    Add(Forum),
    Login {
        forum: Forum,
        user: String,
        password: String,
    },
    Logout(Forum),
    Remove(Forum),
}

pub enum Done {
    Found {
        query: String,
        results: Vec<Forum>,
    },
    /// The store changed: what to say, the forums as they now are, and which to highlight.
    Changed {
        message: String,
        forums: Vec<Forum>,
        focus: Option<String>,
    },
    Failed(String),
}

/// What the card wants next.
// Made per key and dropped at once; boxing the big variant would only add noise.
#[allow(clippy::large_enum_variant)]
pub enum Next {
    Stay,
    Run(Job),
    /// Show this above the card, which stays open.
    Note(String),
    Error(String),
    Close,
}

pub fn run(app: &App, job: Job) -> BoxFuture<'_, Done> {
    Box::pin(async move {
        let (said, focus) = match job {
            Job::Search(query) => {
                let limit = limits::get().forums.directory_results.value;
                return match directory::search(&app.fetch, &query, limit).await {
                    Ok(results) => Done::Found { query, results },
                    Err(e) => Done::Failed(format!("{e:#}")),
                };
            }
            Job::Add(forum) => {
                let focus = forum.base_url.clone();
                (keep(app, Scope::Session, forum), Some(focus))
            }
            Job::Login {
                forum,
                user,
                password,
            } => (
                sign_in(app, &forum, &user, &password).await,
                Some(forum.base_url),
            ),
            Job::Logout(forum) => (sign_out(app, &forum), Some(forum.base_url)),
            Job::Remove(forum) => (forget(app, &forum), None),
        };
        match said.and_then(|message| Ok((message, store::load(&app.paths)?))) {
            Ok((message, forums)) => Done::Changed {
                message,
                forums,
                focus,
            },
            Err(e) => Done::Failed(format!("{e:#}")),
        }
    })
}

impl Forums {
    /// The list, or the login for the forum with base URL `login` when one was named.
    pub fn new(forums: Vec<Forum>, login: Option<&str>) -> Self {
        let at = login.and_then(|url| forums.iter().position(|f| f.base_url == url));
        Self {
            forums,
            selected: at.unwrap_or(0),
            stage: match at {
                Some(_) => username(""),
                None => Stage::List,
            },
        }
    }

    pub fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        match &mut self.stage {
            Stage::List => return self.list_key(action),
            Stage::Menu { choice, acts } => match choice.key(action, key) {
                Step::Stay => {}
                Step::Answer(Reply::Choice(i)) => {
                    let act = acts[i];
                    return self.act(act);
                }
                Step::Answer(_) => self.stage = Stage::List,
            },
            Stage::Add { choice, queries } => match choice.key(action, key) {
                Step::Stay => {}
                Step::Answer(Reply::Choice(i)) => {
                    let query = queries[i].clone();
                    return self.search(query);
                }
                Step::Answer(Reply::Text(words)) => return self.search(words),
                Step::Answer(_) => self.stage = Stage::List,
            },
            Stage::Found {
                results, selected, ..
            } => match action {
                Some(Action::Up) => *selected = selected.saturating_sub(1),
                Some(Action::Down) => *selected = (*selected + 1).min(results.len() - 1),
                Some(Action::Right | Action::Submit) => {
                    let forum = results[*selected].clone();
                    let doing = fill(&ui::get().forums.adding, &[("name", &forum.name)]);
                    return self.work(doing, Back::Add, Job::Add(forum));
                }
                Some(Action::Left | Action::Escape) => self.stage = add(&self.forums),
                _ => {}
            },
            Stage::Username(input) => match action {
                Some(Action::Submit) => {
                    let user = input.text().trim().to_owned();
                    if !user.is_empty() {
                        self.stage = Stage::Password {
                            user,
                            // Dots only: unlike an account number's, a password's tail stays hidden.
                            input: Editor::new(0).masked(0),
                        };
                    }
                }
                Some(Action::Escape) => self.stage = Stage::List,
                Some(other) => {
                    input.apply(other);
                }
                None => input.type_key(key),
            },
            Stage::Password { user, input } => match action {
                Some(Action::Submit) => {
                    // A paste that ended in a newline still means just the password.
                    let password = input.text().trim_end_matches(['\r', '\n']).to_owned();
                    if password.is_empty() {
                        return Next::Stay;
                    }
                    let user = user.clone();
                    let forum = self.forums[self.selected].clone();
                    let doing = fill(
                        &ui::get().forums.logging_in,
                        &[("name", &forum.name), ("user", &user)],
                    );
                    let back = Back::Username(user.clone());
                    return self.work(
                        doing,
                        back,
                        Job::Login {
                            forum,
                            user,
                            password,
                        },
                    );
                }
                Some(Action::Escape) => self.stage = username(&user.clone()),
                Some(other) => {
                    input.apply(other);
                }
                None => input.type_key(key),
            },
            // Only cancelling is left once the forum is being asked.
            Stage::Working { .. } => {
                if matches!(action, Some(Action::Escape | Action::CtrlC)) {
                    return Next::Close;
                }
            }
        }
        Next::Stay
    }

    fn list_key(&mut self, action: Option<Action>) -> Next {
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => self.selected = (self.selected + 1).min(self.forums.len()),
            Some(Action::Right | Action::Submit) => {
                self.stage = match self.forums.get(self.selected) {
                    Some(forum) => menu(forum),
                    None => add(&self.forums),
                };
            }
            Some(Action::Left | Action::Escape) => return Next::Close,
            _ => {}
        }
        Next::Stay
    }

    fn act(&mut self, act: Act) -> Next {
        let forum = self.forums[self.selected].clone();
        let saving = ui::get().forums.saving.clone();
        match act {
            Act::LogIn => {
                self.stage = username("");
                Next::Stay
            }
            Act::LogOut => self.work(saving, Back::List, Job::Logout(forum)),
            Act::Remove => self.work(saving, Back::List, Job::Remove(forum)),
        }
    }

    fn search(&mut self, query: String) -> Next {
        let doing = fill(&ui::get().forums.searching, &[("query", &query)]);
        self.work(doing, Back::Add, Job::Search(query))
    }

    fn work(&mut self, doing: String, back: Back, job: Job) -> Next {
        self.stage = Stage::Working { doing, back };
        Next::Run(job)
    }

    /// Typed or pasted text, for whichever line is open.
    pub fn insert(&mut self, text: &str) {
        match &mut self.stage {
            Stage::Username(input) | Stage::Password { input, .. } => input.insert(text),
            Stage::Menu { choice, .. } | Stage::Add { choice, .. } => choice.insert(text),
            _ => {}
        }
    }

    pub fn finished(&mut self, done: Done) -> Next {
        let text = &ui::get().forums;
        let back = match std::mem::replace(&mut self.stage, Stage::List) {
            Stage::Working { back, .. } => back,
            other => {
                self.stage = other;
                return Next::Stay;
            }
        };
        match done {
            Done::Found { query, results } if results.is_empty() => {
                self.stage = add(&self.forums);
                Next::Error(fill(&text.no_match, &[("query", &query)]))
            }
            Done::Found { query, results } => {
                self.stage = Stage::Found {
                    query,
                    results,
                    selected: 0,
                };
                Next::Stay
            }
            Done::Changed {
                message,
                forums,
                focus,
            } => {
                self.forums = forums;
                self.selected = focus
                    .and_then(|url| self.forums.iter().position(|f| f.base_url == url))
                    .unwrap_or(self.selected)
                    .min(self.forums.len());
                Next::Note(message)
            }
            Done::Failed(error) => {
                self.stage = match back {
                    Back::List => Stage::List,
                    Back::Add => add(&self.forums),
                    Back::Username(user) => username(&user),
                };
                Next::Error(fill(&text.failed, &[("error", &error)]))
            }
        }
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let text = &ui::get().forums;
        let muted = Style::fg("muted");
        let forum = || &self.forums[self.selected];
        let login_title = || fill(&text.login_title, &[("name", &forum().name)]);
        let (title, body, hint) = match &self.stage {
            Stage::Menu { choice, .. } | Stage::Add { choice, .. } => return choice.render(width),
            Stage::List => {
                let mut rows: Vec<Line> = rows(&self.forums, width);
                rows.push(Line::styled(&text.add, Style::fg("accent")));
                let mut body = picker::marked(rows, self.selected);
                body.extend(hint_of(self.forums.get(self.selected)));
                (text.title.clone(), body, &text.hint)
            }
            Stage::Found {
                query,
                results,
                selected,
            } => {
                let mut body = picker::marked(rows(results, width), *selected);
                body.extend(hint_of(results.get(*selected)));
                let title = fill(&text.found_title, &[("query", query)]);
                (title, body, &text.found_hint)
            }
            Stage::Username(input) => (login_title(), login(None, input, width), &text.input_hint),
            Stage::Password { user, input } => (
                login_title(),
                login(Some(user), input, width),
                &text.input_hint,
            ),
            Stage::Working { doing, .. } => (
                text.title.clone(),
                vec![Line::styled(doing, muted.clone())],
                &text.cancel_hint,
            ),
        };
        let mut lines = boxed(Line::styled(title, Style::default().bold()), &body, width);
        lines.push(Line::styled(format!(" {hint}"), muted).truncate(width));
        lines
    }
}

fn username(typed: &str) -> Stage {
    let mut input = Editor::new(0);
    input.set_text(typed);
    Stage::Username(input)
}

fn menu(forum: &Forum) -> Stage {
    let text = &ui::get().forums;
    let acts = match forum.username {
        Some(_) => vec![Act::LogIn, Act::LogOut, Act::Remove],
        None => vec![Act::LogIn, Act::Remove],
    };
    let options = acts
        .iter()
        .enumerate()
        .map(|(i, act)| {
            let label = match (act, &forum.username) {
                (Act::LogIn, None) => &text.log_in,
                (Act::LogIn, Some(_)) => &text.log_in_again,
                (Act::LogOut, _) => &text.log_out,
                (Act::Remove, _) => &text.remove,
            };
            Opt::index(label.clone(), i)
        })
        .collect();
    let mut body = vec![status(forum)];
    body.extend(hint_of(Some(forum)));
    let choice = Choice::new(0, forum.name.clone(), body, options).with_hint(&text.menu_hint);
    Stage::Menu { choice, acts }
}

/// The searches `data/forums.toml` suggests, less the forums already added.
fn add(forums: &[Forum]) -> Stage {
    let text = &ui::get().forums;
    let queries: Vec<String> = forums::data()
        .suggest
        .iter()
        .map(|s| s.query.clone())
        .filter(|q| !forums.iter().any(|f| store::matches(f, q)))
        .collect();
    let mut options: Vec<Opt> = queries
        .iter()
        .enumerate()
        .map(|(i, q)| Opt::index(q.clone(), i))
        .collect();
    options.push(Opt::text(text.search.clone()));
    let body = vec![Line::styled(&text.add_note, Style::fg("muted"))];
    let choice = Choice::new(0, text.add_title.clone(), body, options).with_hint(&text.menu_hint);
    Stage::Add { choice, queries }
}

/// One aligned row per forum: its name, host and whether it is logged in.
fn rows(forums: &[Forum], width: usize) -> Vec<Line> {
    let cells: Vec<Vec<Line>> = forums
        .iter()
        .map(|f| {
            vec![
                Line::plain(&f.name),
                Line::styled(f.host(), Style::fg("muted")),
                status(f),
            ]
        })
        .collect();
    // The host goes first when the row does not fit; the box's borders take four cells.
    let inner = width.saturating_sub(4 + MARKER);
    table::plain(&cells, &[Align::Left; 3], &[1], inner, GAP)
}

fn status(forum: &Forum) -> Line {
    let text = &ui::get().forums;
    match &forum.username {
        Some(user) => Line::styled(fill(&text.logged_in, &[("user", user)]), Style::fg("done")),
        None => Line::styled(&text.guest, Style::fg("muted")),
    }
}

/// What the forum is about, under the list, cut to one line.
fn hint_of(forum: Option<&Forum>) -> Vec<Line> {
    match forum {
        Some(f) if !f.hint.is_empty() => {
            vec![Line::default(), Line::styled(&f.hint, Style::fg("muted"))]
        }
        _ => Vec::new(),
    }
}

/// The login steps: the username once given, then the line being typed.
fn login(user: Option<&str>, input: &Editor, width: usize) -> Vec<Line> {
    let text = &ui::get().forums;
    let inner = width.saturating_sub(4);
    let mut body = vec![
        Line::styled(&text.login_note, Style::fg("muted")),
        Line::default(),
    ];
    let label = match user {
        Some(user) => {
            let mut line = Line::plain(&text.username);
            line.push(user, Style::default().bold());
            body.push(line);
            &text.password
        }
        None => &text.username,
    };
    body.extend(input.render(inner, 1, &Line::plain(label), ""));
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::{KeyCode, KeyEventKind, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn key(card: &mut Forums, action: Action) -> Next {
        card.key(Some(action), &press(KeyCode::Null))
    }

    fn digit(card: &mut Forums, n: char) -> Next {
        card.key(None, &press(KeyCode::Char(n)))
    }

    fn shown(card: &Forums) -> String {
        card.render(80)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn forum(name: &str, user: Option<&str>) -> Forum {
        Forum {
            name: name.into(),
            base_url: format!("https://{}.example/forum", name.to_lowercase()),
            hint: format!("All about {name}."),
            kind: "mobiquo".into(),
            username: user.map(str::to_owned),
            ..Forum::default()
        }
    }

    #[test]
    fn the_list_shows_each_forum_and_whether_it_is_logged_in() {
        let card = Forums::new(
            vec![forum("Alpha", None), forum("Beta", Some("crow"))],
            None,
        );
        let text = shown(&card);
        assert!(text.starts_with("╭ Forums"), "{text}");
        assert!(text.contains("› Alpha"), "{text}");
        assert!(text.contains("alpha.example"), "{text}");
        assert!(text.contains("logged in as crow"), "{text}");
        assert!(text.contains("+ Add a forum"), "{text}");
        assert!(text.contains("All about Alpha."), "{text}");
        assert!(card.render(40).iter().all(|l| l.width() <= 40));
    }

    #[test]
    fn logging_in_asks_a_name_then_a_hidden_password_and_retries_a_refusal() {
        let mut card = Forums::new(vec![forum("Alpha", None)], None);
        key(&mut card, Action::Submit);
        assert!(shown(&card).contains("1 Log in"));
        assert!(matches!(digit(&mut card, '1'), Next::Stay));
        card.insert("crow");
        key(&mut card, Action::Submit);
        card.insert("hunter2");
        let text = shown(&card);
        assert!(text.contains("Username: crow"), "{text}");
        assert!(text.contains("•••••••"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        match key(&mut card, Action::Submit) {
            Next::Run(Job::Login { user, password, .. }) => {
                assert_eq!((user.as_str(), password.as_str()), ("crow", "hunter2"));
            }
            _ => panic!("expected a login"),
        }
        assert!(shown(&card).contains("Logging in to Alpha as crow"));
        match card.finished(Done::Failed("Wrong password".into())) {
            Next::Error(error) => assert!(error.contains("Wrong password"), "{error}"),
            _ => panic!("expected the refusal"),
        }
        // Back at the name, already typed, so a second try is Enter and the password.
        key(&mut card, Action::Submit);
        assert!(shown(&card).contains("Username: crow"));
    }

    #[test]
    fn a_change_returns_to_the_list_as_it_now_is() {
        let mut card = Forums::new(
            vec![forum("Alpha", None)],
            Some("https://alpha.example/forum"),
        );
        assert!(shown(&card).contains("Log in to Alpha"));
        card.insert("crow");
        key(&mut card, Action::Submit);
        card.insert("pw");
        key(&mut card, Action::Submit);
        let next = card.finished(Done::Changed {
            message: "Logged in to Alpha as crow.".into(),
            forums: vec![forum("Alpha", Some("crow"))],
            focus: Some("https://alpha.example/forum".into()),
        });
        assert!(matches!(next, Next::Note(m) if m.contains("Logged in")));
        let text = shown(&card);
        assert!(
            text.contains("› Alpha") && text.contains("logged in as crow"),
            "{text}"
        );
        // A logged-in forum can be logged out of.
        key(&mut card, Action::Submit);
        assert!(matches!(digit(&mut card, '2'), Next::Run(Job::Logout(_))));
    }

    #[test]
    fn adding_offers_suggestions_not_yet_added_and_picks_from_what_a_search_found() {
        let mut card = Forums::new(vec![forum("UnknownCheats", None)], None);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        let text = shown(&card);
        assert!(text.contains("Add a forum"), "{text}");
        assert!(!text.contains("unknowncheats"), "{text}");
        assert!(text.contains("1 mpgh"), "{text}");
        assert!(matches!(digit(&mut card, '1'), Next::Run(Job::Search(q)) if q == "mpgh"));
        card.finished(Done::Found {
            query: "mpgh".into(),
            results: vec![forum("MPGH", None), forum("Other", None)],
        });
        assert!(shown(&card).contains("Forums matching “mpgh”"));
        key(&mut card, Action::Down);
        match key(&mut card, Action::Submit) {
            Next::Run(Job::Add(found)) => assert_eq!(found.name, "Other"),
            _ => panic!("expected an add"),
        }
    }

    #[test]
    fn a_search_that_finds_nothing_says_so_and_escape_walks_back_out() {
        let mut card = Forums::new(Vec::new(), None);
        key(&mut card, Action::Submit);
        let search = (forums::data().suggest.len() + 1).to_string();
        digit(&mut card, search.chars().next().unwrap());
        card.insert("nowhere");
        assert!(
            matches!(key(&mut card, Action::Submit), Next::Run(Job::Search(q)) if q == "nowhere")
        );
        let next = card.finished(Done::Found {
            query: "nowhere".into(),
            results: Vec::new(),
        });
        assert!(matches!(next, Next::Error(e) if e.contains("nowhere")));
        assert!(matches!(key(&mut card, Action::Escape), Next::Stay));
        assert!(matches!(key(&mut card, Action::Escape), Next::Close));
    }
}
