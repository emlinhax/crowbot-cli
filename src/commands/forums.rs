use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::app::App;
use crate::forums::{self, Forum, directory, mobiquo, store};
use crate::io::term;
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/forums.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/forums.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    empty: String,
    heading: String,
    row_guest: String,
    row_login: String,
    added: String,
    not_found: String,
    cancelled: String,
    logged_in: String,
    logged_out: String,
    removed: String,
    username_prompt: String,
    password_prompt: String,
    no_results: String,
}

pub struct Forums;

impl Command for Forums {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let words: Vec<&str> = args.iter().map(String::as_str).collect();
            let session = cx.scope == Scope::Session;
            let text = match words.as_slice() {
                // In a session the list is a card to act on; `list` still prints it.
                [] if session => return Ok(card(None)),
                [] | ["list"] => list(cx),
                ["add"] => add_hint(cx),
                ["add", rest @ ..] => add(cx, &rest.join(" ")).await?,
                ["login", key] if session => {
                    let forum = find_or_add(cx, key).await?;
                    return Ok(card(Some(forum.base_url)));
                }
                ["login", key] => login(cx, key).await?,
                ["logout", key] => act(cx, key, sign_out)?,
                ["remove", key] => act(cx, key, forget)?,
                ["search", key, rest @ ..] if !rest.is_empty() => {
                    search(cx, key, &rest.join(" ")).await?
                }
                _ => return Err(cx.usage(&SPEC)),
            };
            Ok(text.into())
        })
    }
}

fn card(login: Option<String>) -> Outcome {
    Outcome {
        text: String::new(),
        effects: vec![Effect::Forums { login }],
    }
}

fn list(cx: &Ctx<'_>) -> String {
    let forums = store::load(&cx.app.paths).unwrap_or_default();
    if forums.is_empty() {
        return fill(
            &TEXT.empty,
            &[("add", &cx.scope.invoke("forums add <url>"))],
        );
    }
    let mut out = TEXT.heading.clone();
    for f in &forums {
        out.push('\n');
        out.push_str(&match &f.username {
            Some(user) => fill(
                &TEXT.row_login,
                &[("name", &f.name), ("host", &f.host()), ("user", user)],
            ),
            None => fill(&TEXT.row_guest, &[("name", &f.name), ("host", &f.host())]),
        });
        if !f.hint.is_empty() {
            out.push_str(&format!("\n    {}", f.hint));
        }
    }
    out
}

fn add_hint(cx: &Ctx<'_>) -> String {
    let suggestions: Vec<&str> = forums::data()
        .suggest
        .iter()
        .map(|s| s.query.as_str())
        .collect();
    format!(
        "Add a forum by URL or name, e.g. `{}`. Try: {}.",
        cx.scope.invoke("forums add unknowncheats"),
        suggestions.join(", ")
    )
}

async fn add(cx: &Ctx<'_>, query: &str) -> anyhow::Result<String> {
    let forum = directory::resolve(&cx.app.fetch, query).await?;
    keep(cx.app, cx.scope, forum)
}

/// Stores a forum the directory found, and says how to log in to it.
pub fn keep(app: &App, scope: Scope, forum: Forum) -> anyhow::Result<String> {
    store::add(&app.paths, forum.clone())?;
    Ok(fill(
        &TEXT.added,
        &[
            ("name", &forum.name),
            ("host", &forum.host()),
            (
                "login",
                &scope.invoke(&format!("forums login {}", forum.name)),
            ),
        ],
    ))
}

/// The stored forum `key` names, added from the directory first when it is only named, so there
/// is an endpoint to log in to.
async fn find_or_add(cx: &Ctx<'_>, key: &str) -> anyhow::Result<Forum> {
    if let Some(forum) = store::get(&cx.app.paths, key)? {
        return Ok(forum);
    }
    let forum = directory::resolve(&cx.app.fetch, key).await?;
    store::add(&cx.app.paths, forum.clone())?;
    Ok(forum)
}

async fn login(cx: &Ctx<'_>, key: &str) -> anyhow::Result<String> {
    if !term::stdin_is_terminal() {
        anyhow::bail!("logging in needs a terminal");
    }
    let forum = find_or_add(cx, key).await?;
    term::out(&fill(&TEXT.username_prompt, &[("name", &forum.name)]));
    let user = term::read_line()?.unwrap_or_default();
    if user.trim().is_empty() {
        return Ok(TEXT.cancelled.clone());
    }
    let password = match term::read_secret(&TEXT.password_prompt)? {
        Some(password) if !password.is_empty() => password,
        _ => return Ok(TEXT.cancelled.clone()),
    };
    sign_in(cx.app, &forum, user.trim(), &password).await
}

/// Logs in with the user's own account, keeping only the session cookies the forum hands back.
pub async fn sign_in(
    app: &App,
    forum: &Forum,
    user: &str,
    password: &str,
) -> anyhow::Result<String> {
    let login = mobiquo::login(&app.fetch, forum, user, password).await?;
    let user = login.username.clone();
    store::set_login(&app.paths, &forum.base_url, login)?;
    Ok(fill(
        &TEXT.logged_in,
        &[("name", &forum.name), ("user", &user)],
    ))
}

pub fn sign_out(app: &App, forum: &Forum) -> anyhow::Result<String> {
    store::logout(&app.paths, &forum.base_url)?;
    Ok(fill(&TEXT.logged_out, &[("name", &forum.name)]))
}

pub fn forget(app: &App, forum: &Forum) -> anyhow::Result<String> {
    store::remove(&app.paths, &forum.base_url)?;
    Ok(fill(&TEXT.removed, &[("name", &forum.name)]))
}

fn act(
    cx: &Ctx<'_>,
    key: &str,
    act: fn(&App, &Forum) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    match store::get(&cx.app.paths, key)? {
        Some(forum) => act(cx.app, &forum),
        None => Ok(not_found(cx, key)),
    }
}

async fn search(cx: &Ctx<'_>, key: &str, query: &str) -> anyhow::Result<String> {
    let Some(forum) = store::get(&cx.app.paths, key)? else {
        return Ok(not_found(cx, key));
    };
    let list = mobiquo::search(&cx.app.fetch, &forum, query, 1).await?;
    if list.topics.is_empty() {
        return Ok(TEXT.no_results.clone());
    }
    let mut out = String::new();
    for t in &list.topics {
        out.push_str(&format!("[{}] {} — {}\n", t.id, t.title, t.author));
    }
    Ok(out)
}

fn not_found(cx: &Ctx<'_>, key: &str) -> String {
    fill(
        &TEXT.not_found,
        &[
            ("forum", key),
            ("list", &cx.scope.invoke("forums list")),
            ("add", &cx.scope.invoke("forums add <url>")),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_and_text_parse() {
        // Forces both LazyLocks, so the TOML is validated.
        assert_eq!(SPEC.name, "forums");
        assert!(!TEXT.heading.is_empty());
        assert!(SPEC.names().any(|n| n == "forum"));
    }
}
