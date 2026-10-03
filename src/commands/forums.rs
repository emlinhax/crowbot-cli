use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Scope, Spec};
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
    needs_terminal: String,
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
            let text = match words.as_slice() {
                [] | ["list"] => list(cx),
                ["add"] => add_hint(cx),
                ["add", rest @ ..] => add(cx, &rest.join(" ")).await?,
                ["login", forum] => login(cx, forum).await?,
                ["logout", forum] => toggle(cx, forum, store::logout, &TEXT.logged_out)?,
                ["remove", forum] => toggle(cx, forum, store::remove, &TEXT.removed)?,
                ["search", forum, rest @ ..] if !rest.is_empty() => {
                    search(cx, forum, &rest.join(" ")).await?
                }
                _ => return Err(cx.usage(&SPEC)),
            };
            Ok(text.into())
        })
    }
}

fn host(forum: &Forum) -> String {
    url::Url::parse(&forum.base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| forum.name.to_lowercase())
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
                &[("name", &f.name), ("host", &host(f)), ("user", user)],
            ),
            None => fill(&TEXT.row_guest, &[("name", &f.name), ("host", &host(f))]),
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
    store::add(&cx.app.paths, forum.clone())?;
    Ok(fill(
        &TEXT.added,
        &[
            ("name", &forum.name),
            ("host", &host(&forum)),
            (
                "login",
                &cx.scope.invoke(&format!("forums login {}", forum.name)),
            ),
        ],
    ))
}

async fn login(cx: &Ctx<'_>, key: &str) -> anyhow::Result<String> {
    if cx.scope == Scope::Session {
        return Ok(fill(&TEXT.needs_terminal, &[("forum", key)]));
    }
    if !term::stdin_is_terminal() {
        anyhow::bail!("logging in needs a terminal");
    }
    // Add the forum first if it is only named, so there is an endpoint to log in to.
    let forum = match store::get(&cx.app.paths, key)? {
        Some(forum) => forum,
        None => {
            let forum = directory::resolve(&cx.app.fetch, key).await?;
            store::add(&cx.app.paths, forum.clone())?;
            forum
        }
    };

    term::out(&fill(&TEXT.username_prompt, &[("name", &forum.name)]));
    let user = term::read_line()?.unwrap_or_default();
    if user.trim().is_empty() {
        return Ok(TEXT.cancelled.clone());
    }
    let password = match term::read_secret(&TEXT.password_prompt)? {
        Some(password) if !password.is_empty() => password,
        _ => return Ok(TEXT.cancelled.clone()),
    };

    let login = mobiquo::login(&cx.app.fetch, &forum, user.trim(), &password).await?;
    store::set_login(&cx.app.paths, &forum.base_url, login.clone())?;
    Ok(fill(
        &TEXT.logged_in,
        &[("name", &forum.name), ("user", &login.username)],
    ))
}

fn toggle(
    cx: &Ctx<'_>,
    key: &str,
    act: fn(&crate::paths::Paths, &str) -> anyhow::Result<bool>,
    done: &str,
) -> anyhow::Result<String> {
    let Some(forum) = store::get(&cx.app.paths, key)? else {
        return Ok(not_found(cx, key));
    };
    act(&cx.app.paths, key)?;
    Ok(fill(done, &[("name", &forum.name)]))
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
