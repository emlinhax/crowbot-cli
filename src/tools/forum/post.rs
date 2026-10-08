//! The one forum tool that writes. The user sees the exact text and confirms every post, even in
//! AUTO.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{TEXT, find};
use crate::forums::{Forum, mobiquo};
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::template::fill;
use crate::tools::{Check, Output, Refusal, Spec, Tool, ToolCx};

pub struct Post;

static POST: LazyLock<Spec> = crate::tools::spec!("forum_post");

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
        let user = forum.username.as_deref().unwrap_or("?");
        match &args.topic {
            Some(topic) => fill(
                &TEXT.reply_preview,
                &[
                    ("topic", topic),
                    ("forum", &forum.name),
                    ("user", user),
                    ("body", &args.body),
                ],
            ),
            None => fill(
                &TEXT.topic_preview,
                &[
                    ("forum", &forum.name),
                    ("user", user),
                    ("subject", args.subject.as_deref().unwrap_or("")),
                    ("body", &args.body),
                ],
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
        let forum = find(cx, &args.forum)?;
        let ask = Ask::new(catalog::FORUM_POST.name, forum.host());
        Ok(Check::new(vec![ask])
            .with_preview(Self::preview(&forum, args))
            .confirmed())
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let forum = find(cx, &args.forum)?;
            let posted = match &args.topic {
                Some(topic) => {
                    mobiquo::reply(&cx.app.fetch, &forum, &args.section, topic, &args.body).await
                }
                None => {
                    let subject = args.subject.as_deref().unwrap_or("");
                    mobiquo::new_topic(&cx.app.fetch, &forum, &args.section, subject, &args.body)
                        .await
                }
            }
            .map_err(|e| format!("{e:#}"))?;
            let extra = posted
                .url
                .map(|url| fill(&TEXT.posted_url, &[("url", &url)]))
                .or_else(|| posted.id.map(|id| fill(&TEXT.posted_id, &[("id", &id)])))
                .unwrap_or_default();
            Ok(Output::ok(fill(
                &TEXT.posted,
                &[("forum", &forum.name), ("extra", &extra)],
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
