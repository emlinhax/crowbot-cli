//! The read-only forum tools: sections, topics, threads and search. Each takes a batch of ids.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{TEXT, find, render};
use crate::config::limits;
use crate::forums::{Page, mobiquo, store};
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;
use crate::text::truncate::{self, Keep};
use crate::tools::many::{Id, Many, batch};
use crate::tools::{Check, Output, Refusal, Spec, Tool, ToolCx};

/// What a read-only call asks for: the stored forum's host when known, so rules match the host
/// that is reached.
fn browse_check(cx: &ToolCx<'_>, key: &str) -> Check {
    let host = if key.is_empty() {
        "*".to_owned()
    } else {
        find(cx, key).map_or_else(|_| key.to_lowercase(), |f| f.host())
    };
    Check::new(vec![Ask::new(catalog::FORUM.name, host)])
}

fn page_of(page: &Value) -> i64 {
    page.as_i64().unwrap_or(1)
}

/// Reads `page`: a positive number, or "last"/"latest" (also a number ≤ 0) for the final page.
fn page_pref(page: &Value) -> Page {
    match page {
        Value::String(s) => {
            let s = s.trim().to_lowercase();
            if matches!(s.as_str(), "last" | "latest" | "newest" | "end") {
                Page::Last
            } else {
                match s.parse::<i64>() {
                    Ok(n) if n >= 1 => Page::Num(n),
                    Ok(_) => Page::Last,
                    Err(_) => Page::Num(1),
                }
            }
        }
        Value::Number(n) => match n.as_i64().unwrap_or(1) {
            n if n >= 1 => Page::Num(n),
            _ => Page::Last,
        },
        _ => Page::Num(1),
    }
}

/// Cut long output to the tool limits, noting when it was trimmed.
fn finish(text: String) -> Output {
    let limits = &limits::get().tools;
    let cut = truncate::cut(
        &text,
        Keep::Head,
        limits.max_lines.value,
        limits.max_bytes.value,
    );
    let mut content = cut.text.clone();
    if cut.truncated() {
        content.push_str("\n\n");
        content.push_str(&fill(
            &crate::tools::text().cut,
            &[
                ("kept", &cut.kept_lines.to_string()),
                ("total", &cut.total_lines.to_string()),
            ],
        ));
    }
    Output::ok(content)
}

/// A batch arg's ids as strings, capped to the batch limit so one call cannot hammer a forum.
fn capped(many: &Many<Id>) -> Vec<String> {
    many.capped(limits::get().forums.batch_max.value)
        .iter()
        .map(Id::text)
        .collect()
}

/// Each id read at once, under its label, and the whole cut to the tool limits.
async fn read_each<F, Fut>(ids: &Many<Id>, label: impl Fn(&str) -> String, read: F) -> Output
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    finish(batch(capped(ids), label, read).await)
}

pub struct Sections;

static SECTIONS: LazyLock<Spec> = crate::tools::spec!("forum_sections");

#[derive(Deserialize)]
pub struct SectionsArgs {
    #[serde(default)]
    forum: String,
}

impl Tool for Sections {
    type Args = SectionsArgs;

    fn spec(&self) -> &Spec {
        &SECTIONS
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(browse_check(cx, &args.forum))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            if args.forum.is_empty() {
                let list = store::load(&cx.app.paths).unwrap_or_default();
                return Ok(finish(render::forums(&list)));
            }
            let forum = find(cx, &args.forum)?;
            let sections = mobiquo::sections(&cx.app.fetch, &forum)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let mut text = render::sections(&sections);
            if !forum.hint.is_empty() {
                text = format!("{} — {}\n\n{text}", forum.name, forum.hint);
            }
            Ok(finish(text))
        })
    }
}

pub struct Topics;

static TOPICS: LazyLock<Spec> = crate::tools::spec!("forum_topics");

#[derive(Deserialize)]
pub struct TopicsArgs {
    forum: String,
    section: Many<Id>,
    #[serde(default)]
    page: Value,
}

impl Tool for Topics {
    type Args = TopicsArgs;

    fn spec(&self) -> &Spec {
        &TOPICS
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(browse_check(cx, &args.forum))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let forum = find(cx, &args.forum)?;
            let page = page_of(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            Ok(read_each(
                &args.section,
                |section| fill(&TEXT.section_label, &[("id", section)]),
                |section| async move {
                    let list = mobiquo::topics(fetch, forum, &section, page).await?;
                    Ok(render::topics(&list))
                },
            )
            .await)
        })
    }
}

pub struct ThreadTool;

static THREAD: LazyLock<Spec> = crate::tools::spec!("forum_thread");

#[derive(Deserialize)]
pub struct ThreadArgs {
    forum: String,
    topic: Many<Id>,
    #[serde(default)]
    page: Value,
}

impl Tool for ThreadTool {
    type Args = ThreadArgs;

    fn spec(&self) -> &Spec {
        &THREAD
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(browse_check(cx, &args.forum))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let forum = find(cx, &args.forum)?;
            let want = page_pref(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            Ok(read_each(
                &args.topic,
                |topic| fill(&TEXT.topic_label, &[("id", topic)]),
                |topic| async move {
                    let thread = mobiquo::thread(fetch, forum, &topic, want).await?;
                    Ok(render::thread(&thread))
                },
            )
            .await)
        })
    }
}

pub struct Search;

static SEARCH: LazyLock<Spec> = crate::tools::spec!("forum_search");

#[derive(Deserialize)]
pub struct SearchArgs {
    forum: String,
    query: Many<Id>,
    #[serde(default)]
    page: Value,
}

impl Tool for Search {
    type Args = SearchArgs;

    fn spec(&self) -> &Spec {
        &SEARCH
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        Ok(browse_check(cx, &args.forum))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let forum = find(cx, &args.forum)?;
            let page = page_of(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            Ok(read_each(
                &args.query,
                |query| fill(&TEXT.query_label, &[("query", query)]),
                |query| async move {
                    let list = mobiquo::search(fetch, forum, &query, page).await?;
                    Ok(render::topics(&list))
                },
            )
            .await)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_pref_reads_numbers_and_last() {
        let num = |v: Value| matches!(page_pref(&v), Page::Num(_));
        let last = |v: Value| matches!(page_pref(&v), Page::Last);
        assert!(matches!(page_pref(&Value::Null), Page::Num(1)));
        assert!(num(serde_json::json!(3)) && num(serde_json::json!("2")));
        assert!(last(serde_json::json!("last")) && last(serde_json::json!("latest")));
        assert!(last(serde_json::json!(-1)) && last(serde_json::json!(0)));
    }

    #[test]
    fn a_batch_arg_takes_strings_numbers_and_arrays_and_caps() {
        let parse = |v: Value| capped(&serde_json::from_value::<Many<Id>>(v).unwrap());
        assert_eq!(parse(serde_json::json!("a")), ["a"]);
        // A bare number is accepted and coerced, which is how models often send an id.
        assert_eq!(parse(serde_json::json!(42)), ["42"]);
        assert_eq!(parse(serde_json::json!(["a", "b"])), ["a", "b"]);
        assert_eq!(parse(serde_json::json!([1, 2])), ["1", "2"]);
        let big = serde_json::json!((0..50).collect::<Vec<_>>());
        assert_eq!(parse(big).len(), limits::get().forums.batch_max.value);
    }
}
