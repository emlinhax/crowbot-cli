//! Forum replies as the model reads them.

use super::TEXT;
use crate::forums::{Forum, Section, Thread, TopicList};
use crate::text::template::fill;

/// One line per forum, each with its hint under it; `logins` adds who each is browsed as. The
/// system prompt names the forums with this too.
pub fn list(list: &[Forum], logins: bool) -> String {
    let lines: Vec<String> = list
        .iter()
        .map(|f| {
            let mut line = fill(&TEXT.line, &[("name", &f.name), ("host", &f.host())]);
            if logins {
                line.push_str(&match &f.username {
                    Some(user) => fill(&TEXT.logged_in, &[("user", user)]),
                    None => TEXT.guest.clone(),
                });
            }
            if !f.hint.is_empty() {
                line.push('\n');
                line.push_str(&fill(&TEXT.hint, &[("hint", &f.hint)]));
            }
            line
        })
        .collect();
    lines.join("\n")
}

pub fn forums(forums: &[Forum]) -> String {
    if forums.is_empty() {
        return TEXT.no_forums.clone();
    }
    format!("{}\n{}", TEXT.heading, list(forums, true))
}

pub fn sections(sections: &[Section]) -> String {
    if sections.is_empty() {
        return TEXT.no_sections.clone();
    }
    let mut out = String::new();
    for s in sections {
        out.push_str(&"  ".repeat(s.depth));
        out.push_str(&fill(&TEXT.section, &[("id", &s.id), ("name", &s.name)]));
        if s.sub_only {
            out.push_str(&TEXT.sub_only);
        }
        out.push('\n');
    }
    out
}

pub fn topics(list: &TopicList) -> String {
    if list.topics.is_empty() {
        return TEXT.no_topics.clone();
    }
    let mut out = String::new();
    if let Some(total) = list.total {
        out.push_str(&fill(&TEXT.total_topics, &[("total", &total.to_string())]));
        out.push('\n');
    }
    for t in &list.topics {
        let replies = t.replies.to_string();
        out.push_str(&fill(
            &TEXT.topic,
            &[
                ("id", &t.id),
                ("title", &t.title),
                ("author", &t.author),
                ("replies", &replies),
            ],
        ));
        out.push('\n');
    }
    out
}

pub fn thread(thread: &Thread) -> String {
    let mut out = thread.title.clone();
    let page = thread.page;
    if let (Some(total), Some(pages)) = (thread.total, thread.pages) {
        out.push_str(&fill(
            &TEXT.pages,
            &[
                ("page", &page.to_string()),
                ("pages", &pages.to_string()),
                ("total", &total.to_string()),
            ],
        ));
        if page < pages {
            out.push('\n');
            out.push_str(&TEXT.newest);
        }
    }
    out.push('\n');
    for p in &thread.posts {
        out.push('\n');
        out.push_str(&fill(
            &TEXT.post,
            &[("id", &p.id), ("author", &p.author), ("time", &p.time)],
        ));
        out.push('\n');
        out.push_str(p.content.trim_end());
        out.push('\n');
    }
    out
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
        let text = forums(&list);
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
        let text = super::thread(&thread);
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
        let page1 = thread(&page(1));
        assert!(page1.contains("page 1 of 6"), "{page1}");
        assert!(page1.contains(r#"read page "last""#), "{page1}");
        // On the last page the nudge is gone.
        let page6 = thread(&page(6));
        assert!(!page6.contains("newest posts"), "{page6}");
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
        let text = topics(&list);
        assert!(text.contains("3 topics in total"));
        assert!(text.contains("[42] aimbot — crow (7 replies)"));
    }
}
