//! Forum replies as the model reads them.

use super::TEXT;
use crate::forums::{Forum, Section, Thread, TopicList};

pub fn forums(list: &[Forum]) -> String {
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

pub fn sections(sections: &[Section]) -> String {
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

pub fn topics(list: &TopicList) -> String {
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

pub fn thread(thread: &Thread) -> String {
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
