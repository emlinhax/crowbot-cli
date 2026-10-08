//! The forum tools the model can call: browse sections, topics and threads, search, and post. All
//! read-only ones are allowed by default; the posting one shows the user the exact text and waits
//! for a yes every time, even in AUTO. Forum content is untrusted — the descriptions tell the model
//! so, and nothing here acts on it automatically.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::many::{Many, join};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::config::limits;
use crate::data;
use crate::forums::{Forum, Page, Section, Thread, TopicList, mobiquo, store};
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;
use crate::text::truncate::{self, Keep};

static TEXT: LazyLock<Text> = data::load!("tools/forum.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    no_forums: String,
    unknown: String,
    no_subject: String,
    posted: String,
    cut: String,
}

/// The host that a permission rule matches, found from the stored forum when possible.
fn host_for(cx: &ToolCx<'_>, key: &str) -> String {
    if key.is_empty() {
        return "*".to_owned();
    }
    store::get(&cx.app.paths, key)
        .ok()
        .flatten()
        .map(|f| f.host())
        .unwrap_or_else(|| key.to_lowercase())
}

/// Find the forum `key` names, or an error the model can act on.
fn resolve(cx: &ToolCx<'_>, key: &str) -> Result<Forum, Output> {
    match store::get(&cx.app.paths, key) {
        Ok(Some(forum)) => Ok(forum),
        Ok(None) => {
            let list = store::load(&cx.app.paths).unwrap_or_default();
            Err(Output::error(if list.is_empty() {
                TEXT.no_forums.clone()
            } else {
                fill(&TEXT.unknown, &[("forum", key)])
            }))
        }
        Err(e) => Err(Output::error(format!("{e:#}"))),
    }
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
            &TEXT.cut,
            &[
                ("kept", &cut.kept_lines.to_string()),
                ("total", &cut.total_lines.to_string()),
            ],
        ));
    }
    Output::ok(content)
}

/// An id or search word, which the model may send as a string or a bare number.
#[derive(Deserialize)]
#[serde(untagged)]
enum Id {
    Str(String),
    Int(i64),
}

impl Id {
    fn text(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Int(n) => n.to_string(),
        }
    }
}

/// A batch arg's ids as strings, capped to the batch limit so one call cannot hammer a forum.
fn capped(many: &Many<Id>) -> Vec<String> {
    many.capped(limits::get().forums.batch_max.value)
        .iter()
        .map(Id::text)
        .collect()
}

/// Run `each` over the items at once and stitch the results, labelling and inlining per-item
/// errors so one bad id does not sink the batch. A single item gets no label or divider.
async fn batch<F, Fut>(items: Vec<String>, label: impl Fn(&str) -> String, each: F) -> String
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    let each = &each;
    let runs = items
        .into_iter()
        .map(move |item| async move { (item.clone(), each(item).await) });
    let parts = futures_util::future::join_all(runs)
        .await
        .into_iter()
        .map(|(item, result)| {
            let text = result.unwrap_or_else(|e| format!("could not read {item}: {e:#}"));
            (label(&item), text)
        })
        .collect();
    join(parts)
}

fn render_forums(list: &[Forum]) -> String {
    if list.is_empty() {
        return TEXT.no_forums.clone();
    }
    let mut out = String::from("Forums you can browse:\n");
    for f in list {
        let who = match &f.username {
            Some(user) => format!("logged in as {user}"),
            None => "guest".to_owned(),
        };
        out.push_str(&format!("- {} ({}) — {who}\n", f.name, f.host()));
        if !f.hint.is_empty() {
            out.push_str(&format!("  {}\n", f.hint));
        }
    }
    out
}

fn render_sections(sections: &[Section]) -> String {
    if sections.is_empty() {
        return "No sections.".to_owned();
    }
    let mut out = String::new();
    for s in sections {
        let indent = "  ".repeat(s.depth);
        let tail = if s.sub_only { "  (heading)" } else { "" };
        out.push_str(&format!("{indent}[{}] {}{tail}\n", s.id, s.name));
    }
    out
}

fn render_topics(list: &TopicList) -> String {
    if list.topics.is_empty() {
        return "No topics.".to_owned();
    }
    let mut out = String::new();
    if let Some(total) = list.total {
        out.push_str(&format!("{total} topics in total.\n"));
    }
    for t in &list.topics {
        out.push_str(&format!(
            "[{}] {} — {} ({} replies)\n",
            t.id, t.title, t.author, t.replies
        ));
    }
    out
}

fn render_thread(thread: &Thread) -> String {
    let mut out = thread.title.clone();
    let page = thread.page;
    if let (Some(total), Some(pages)) = (thread.total, thread.pages) {
        out.push_str(&format!(" — page {page} of {pages}, {total} posts"));
        // Posts are oldest first; nudge toward the end rather than paging there one by one.
        if page < pages {
            out.push_str("\n(oldest first; for the newest posts read page \"last\")");
        }
    }
    out.push('\n');
    for p in &thread.posts {
        out.push_str(&format!("\n── #{} {} {}\n", p.id, p.author, p.time));
        out.push_str(p.content.trim_end());
        out.push('\n');
    }
    out
}

// --- forum_sections ---------------------------------------------------------

pub struct Sections;

static SECTIONS: LazyLock<Spec> = super::spec!("forum_sections");

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
        Ok(Check::new(vec![Ask::new(
            catalog::FORUM.name,
            host_for(cx, &args.forum),
        )]))
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if args.forum.is_empty() {
                let list = store::load(&cx.app.paths).unwrap_or_default();
                return finish(render_forums(&list));
            }
            let forum = match resolve(cx, &args.forum) {
                Ok(f) => f,
                Err(out) => return out,
            };
            match mobiquo::sections(&cx.app.fetch, &forum).await {
                Ok(sections) => {
                    let mut text = render_sections(&sections);
                    if !forum.hint.is_empty() {
                        text = format!("{} — {}\n\n{text}", forum.name, forum.hint);
                    }
                    finish(text)
                }
                Err(e) => Output::error(format!("{e:#}")),
            }
        })
    }
}

// --- forum_topics -----------------------------------------------------------

pub struct Topics;

static TOPICS: LazyLock<Spec> = super::spec!("forum_topics");

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
        Ok(Check::new(vec![Ask::new(
            catalog::FORUM.name,
            host_for(cx, &args.forum),
        )]))
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let forum = match resolve(cx, &args.forum) {
                Ok(f) => f,
                Err(out) => return out,
            };
            let page = page_of(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            let text = batch(
                capped(&args.section),
                |section| format!("Section {section}:"),
                |section| async move {
                    let list = mobiquo::topics(fetch, forum, &section, page).await?;
                    Ok(render_topics(&list))
                },
            )
            .await;
            finish(text)
        })
    }
}

// --- forum_thread -----------------------------------------------------------

pub struct ThreadTool;

static THREAD: LazyLock<Spec> = super::spec!("forum_thread");

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
        Ok(Check::new(vec![Ask::new(
            catalog::FORUM.name,
            host_for(cx, &args.forum),
        )]))
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let forum = match resolve(cx, &args.forum) {
                Ok(f) => f,
                Err(out) => return out,
            };
            let want = page_pref(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            let text = batch(
                capped(&args.topic),
                |topic| format!("Topic {topic}:"),
                |topic| async move {
                    let thread = mobiquo::thread(fetch, forum, &topic, want).await?;
                    Ok(render_thread(&thread))
                },
            )
            .await;
            finish(text)
        })
    }
}

// --- forum_search -----------------------------------------------------------

pub struct Search;

static SEARCH: LazyLock<Spec> = super::spec!("forum_search");

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
        Ok(Check::new(vec![Ask::new(
            catalog::FORUM.name,
            host_for(cx, &args.forum),
        )]))
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let forum = match resolve(cx, &args.forum) {
                Ok(f) => f,
                Err(out) => return out,
            };
            let page = page_of(&args.page);
            let (fetch, forum) = (&cx.app.fetch, &forum);
            let text = batch(
                capped(&args.query),
                |query| format!("\"{query}\":"),
                |query| async move {
                    let list = mobiquo::search(fetch, forum, &query, page).await?;
                    Ok(render_topics(&list))
                },
            )
            .await;
            finish(text)
        })
    }
}

// --- forum_post -------------------------------------------------------------

pub struct Post;

static POST: LazyLock<Spec> = super::spec!("forum_post");

#[derive(Deserialize)]
pub struct PostArgs {
    forum: String,
    section: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    body: String,
}

impl Post {
    /// What the user sees before anything is sent.
    fn preview(forum: &Forum, args: &PostArgs) -> String {
        match &args.topic {
            Some(topic) => format!(
                "Reply to topic {topic} on {} as {}:\n\n{}",
                forum.name,
                forum.username.as_deref().unwrap_or("?"),
                args.body
            ),
            None => format!(
                "New topic on {} as {}\nSubject: {}\n\n{}",
                forum.name,
                forum.username.as_deref().unwrap_or("?"),
                args.subject.as_deref().unwrap_or(""),
                args.body
            ),
        }
    }
}

impl Tool for Post {
    type Args = PostArgs;

    fn spec(&self) -> &Spec {
        &POST
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if args.topic.is_none() && args.subject.as_deref().unwrap_or("").trim().is_empty() {
            return Err(Refusal::InvalidArgs(TEXT.no_subject.clone()));
        }
        let forum = resolve(cx, &args.forum).map_err(|out| Refusal::Refused(out.content))?;
        let ask = Ask::new(catalog::FORUM_POST.name, forum.host());
        Ok(Check::new(vec![ask])
            .with_preview(Self::preview(&forum, args))
            .confirmed())
    }

    fn run<'a>(&'a self, args: Self::Args, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let forum = match resolve(cx, &args.forum) {
                Ok(f) => f,
                Err(out) => return out,
            };

            let result = match &args.topic {
                Some(topic) => {
                    mobiquo::reply(&cx.app.fetch, &forum, &args.section, topic, &args.body).await
                }
                None => {
                    let subject = args.subject.as_deref().unwrap_or("");
                    mobiquo::new_topic(&cx.app.fetch, &forum, &args.section, subject, &args.body)
                        .await
                }
            };
            match result {
                Ok(posted) => {
                    let extra = posted
                        .url
                        .map(|u| format!(" {u}"))
                        .or_else(|| posted.id.map(|id| format!(" (post {id})")))
                        .unwrap_or_default();
                    Output::ok(fill(
                        &TEXT.posted,
                        &[("forum", &forum.name), ("extra", &extra)],
                    ))
                }
                Err(e) => Output::error(format!("{e:#}")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forums::{Post as ForumPost, Topic};

    #[test]
    fn forums_render_with_their_login_state() {
        let list = vec![
            Forum {
                name: "UnknownCheats".into(),
                hint: "Game hacking and reverse engineering.".into(),
                base_url: "https://www.unknowncheats.me/forum".into(),
                username: Some("crow".into()),
                ..Forum::default()
            },
            Forum {
                name: "Other".into(),
                base_url: "https://other.example".into(),
                ..Forum::default()
            },
        ];
        let text = render_forums(&list);
        assert!(text.contains("UnknownCheats (www.unknowncheats.me) — logged in as crow"));
        assert!(text.contains("  Game hacking and reverse engineering."));
        assert!(text.contains("Other (other.example) — guest"));
    }

    #[test]
    fn a_thread_renders_each_post() {
        let thread = Thread {
            title: "Hi".into(),
            total: Some(2),
            page: 1,
            pages: Some(1),
            posts: vec![ForumPost {
                id: "5".into(),
                author: "crow".into(),
                time: "2026".into(),
                content: "body".into(),
            }],
        };
        let text = render_thread(&thread);
        assert!(text.contains("Hi — page 1 of 1, 2 posts"));
        assert!(text.contains("── #5 crow 2026"));
        assert!(text.contains("body"));
    }

    #[test]
    fn a_thread_footer_shows_pages_and_nudges_toward_the_newest() {
        let page = |page| Thread {
            title: "Long".into(),
            total: Some(101),
            page,
            pages: Some(6),
            posts: vec![],
        };
        let page1 = render_thread(&page(1));
        assert!(page1.contains("page 1 of 6"), "{page1}");
        assert!(page1.contains(r#"read page "last""#), "{page1}");
        // On the last page the nudge is gone.
        let page6 = render_thread(&page(6));
        assert!(!page6.contains("newest posts"), "{page6}");
    }

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
    fn topics_note_the_total_and_each_id() {
        let list = TopicList {
            total: Some(3),
            topics: vec![Topic {
                id: "42".into(),
                title: "aimbot".into(),
                author: "crow".into(),
                replies: 7,
            }],
        };
        let text = render_topics(&list);
        assert!(text.contains("3 topics in total"));
        assert!(text.contains("[42] aimbot — crow (7 replies)"));
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

    #[tokio::test]
    async fn a_batch_labels_each_and_inlines_a_failure() {
        let out = batch(
            vec!["a".into(), "b".into()],
            |x| format!("[{x}]"),
            |x| async move {
                if x == "b" {
                    anyhow::bail!("nope")
                } else {
                    Ok(format!("ok {x}"))
                }
            },
        )
        .await;
        assert!(out.contains("[a]\nok a"), "{out}");
        assert!(out.contains("[b]\ncould not read b: nope"), "{out}");
        assert!(out.contains("───"), "{out}");

        // A single item carries no label or divider.
        let solo = batch(
            vec!["a".into()],
            |x| format!("[{x}]"),
            |x| async move { Ok(format!("ok {x}")) },
        )
        .await;
        assert_eq!(solo, "ok a");
    }

    #[test]
    fn a_new_topic_without_a_subject_is_rejected() {
        let project = crate::tools::testing::Project::new();
        let args = serde_json::json!({"forum": "x", "section": "1", "body": "hi"});
        match project.check(&Post, &args) {
            Err(Refusal::InvalidArgs(_)) => {}
            _ => panic!("a new topic with no subject should be rejected"),
        }
    }
}
