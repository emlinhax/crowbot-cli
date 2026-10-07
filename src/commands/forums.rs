use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::forums::{Forum, account, mobiquo, store};
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
    row: String,
    guest: String,
    member: String,
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
                    let forum = account::find_or_add(&cx.app.fetch, &cx.app.paths, key).await?;
                    return Ok(card(Some(forum.base_url)));
                }
                ["login", key] => login(cx, key).await?,
                ["logout", key] => match store::get(&cx.app.paths, key)? {
                    Some(forum) => {
                        store::logout(&cx.app.paths, &forum.base_url)?;
                        logged_out(&forum)
                    }
                    None => not_found(cx, key),
                },
                ["remove", key] => match store::get(&cx.app.paths, key)? {
                    Some(forum) => {
                        store::remove(&cx.app.paths, &forum.base_url)?;
                        removed(&forum)
                    }
                    None => not_found(cx, key),
                },
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
        out.push_str(&fill(
            &TEXT.row,
            &[
                ("name", &f.name),
                ("host", &f.host()),
                ("status", &status(f)),
            ],
        ));
        if !f.hint.is_empty() {
            out.push_str(&format!("\n    {}", f.hint));
        }
    }
    out
}

fn add_hint(cx: &Ctx<'_>) -> String {
    let example = format!(
        "Add a forum by URL or name, e.g. `{}`.",
        cx.scope.invoke("forums add unknowncheats")
    );
    let stored = store::load(&cx.app.paths).unwrap_or_default();
    match account::suggestions(&stored).as_slice() {
        [] => example,
        suggestions => format!("{example} Try: {}.", suggestions.join(", ")),
    }
}

async fn add(cx: &Ctx<'_>, query: &str) -> anyhow::Result<String> {
    let forum = account::add(&cx.app.fetch, &cx.app.paths, query).await?;
    Ok(added(cx.scope, &forum))
}

/// A forum was stored; how to log in to it.
pub fn added(scope: Scope, forum: &Forum) -> String {
    fill(
        &TEXT.added,
        &[
            ("name", &forum.name),
            ("host", &forum.host()),
            (
                "login",
                &scope.invoke(&format!("forums login {}", forum.name)),
            ),
        ],
    )
}

async fn login(cx: &Ctx<'_>, key: &str) -> anyhow::Result<String> {
    if !term::stdin_is_terminal() {
        anyhow::bail!("logging in needs a terminal");
    }
    let forum = account::find_or_add(&cx.app.fetch, &cx.app.paths, key).await?;
    term::out(&fill(&TEXT.username_prompt, &[("name", &forum.name)]));
    let user = term::read_line()?.unwrap_or_default();
    if user.trim().is_empty() {
        return Ok(TEXT.cancelled.clone());
    }
    let password = match term::read_secret(&TEXT.password_prompt)? {
        Some(password) if !password.is_empty() => password,
        _ => return Ok(TEXT.cancelled.clone()),
    };
    let user =
        account::sign_in(&cx.app.fetch, &cx.app.paths, &forum, user.trim(), &password).await?;
    Ok(logged_in(&forum, &user))
}

/// Guest, or who the user is logged in as.
pub fn status(forum: &Forum) -> String {
    match &forum.username {
        Some(user) => fill(&TEXT.member, &[("user", user)]),
        None => TEXT.guest.clone(),
    }
}

pub fn logged_in(forum: &Forum, user: &str) -> String {
    fill(&TEXT.logged_in, &[("name", &forum.name), ("user", user)])
}

pub fn logged_out(forum: &Forum) -> String {
    fill(&TEXT.logged_out, &[("name", &forum.name)])
}

pub fn removed(forum: &Forum) -> String {
    fill(&TEXT.removed, &[("name", &forum.name)])
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
