//! The mobiquo backend: XML-RPC calls to a forum's Tapatalk plugin, over the browser transport
//! (`io::fetch`) so a Cloudflare WAF sees a real browser's TLS. Strings the plugin wants as
//! `<base64>` (names, passwords, search words, post bodies) go through `Arg::b64`.

use anyhow::{Context, Result, bail};

use crate::forums::xmlrpc::{self, Arg, Value};
use crate::forums::{Forum, Login, Post, Posted, Section, Thread, Topic, TopicList};
use crate::io::fetch::{self, Fetch};
use crate::limits;

fn endpoint(forum: &Forum) -> String {
    let base = forum.base_url.trim_end_matches('/');
    let ext = if forum.ext.is_empty() {
        "php"
    } else {
        &forum.ext
    };
    if forum.mobiquo_dir.is_empty() {
        format!("{base}/mobiquo/mobiquo.{ext}")
    } else {
        format!("{base}/{}/mobiquo.{ext}", forum.mobiquo_dir)
    }
}

/// The `Cookie` header to replay a session: the `name=value` of each stored `Set-Cookie`.
fn cookie_header(cookies: &[String]) -> Option<String> {
    let pairs: Vec<&str> = cookies
        .iter()
        .filter_map(|c| c.split(';').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

/// One call, returning the parsed value and any `Set-Cookie` (which `login` keeps).
async fn send(
    fetch: &Fetch,
    forum: &Forum,
    method: &str,
    params: &[Arg],
) -> Result<(Value, Vec<String>)> {
    let url = endpoint(forum);
    let body = xmlrpc::call(method, params).into_bytes();
    let cookie = cookie_header(&forum.cookies);
    let resp = fetch
        .post(fetch::Post {
            url: &url,
            body,
            content_type: "text/xml",
            cookie: cookie.as_deref(),
        })
        .await
        .with_context(|| format!("reaching {}", forum.name))?;
    if resp.status >= 400 {
        let hint = if resp.firewall {
            " (the forum's firewall refused it)"
        } else {
            ""
        };
        bail!("{} replied HTTP {}{hint}", forum.name, resp.status);
    }
    let value = xmlrpc::parse(&resp.body)
        .with_context(|| format!("reading {method} from {}", forum.name))?;
    Ok((value, resp.set_cookies))
}

/// The forum's error convention: a `result` member set false, carrying why in `result_text`.
fn ok(value: Value) -> Result<Value> {
    if value.get("result").and_then(Value::as_bool) == Some(false) {
        bail!(
            "{}",
            value
                .get_str("result_text")
                .unwrap_or("the forum refused the request")
        );
    }
    Ok(value)
}

async fn call(fetch: &Fetch, forum: &Forum, method: &str, params: &[Arg]) -> Result<Value> {
    let (value, _) = send(fetch, forum, method, params).await?;
    ok(value)
}

fn span(page: i64) -> (i64, i64) {
    let per = limits::get().forums.page_size.value;
    let start = (page.max(1) - 1) * per;
    (start, start + per - 1)
}

pub async fn sections(fetch: &Fetch, forum: &Forum) -> Result<Vec<Section>> {
    let value = call(fetch, forum, "get_forum", &[]).await?;
    let mut out = Vec::new();
    flatten(value.as_array().unwrap_or(&[]), 0, &mut out);
    Ok(out)
}

fn flatten(nodes: &[Value], depth: usize, out: &mut Vec<Section>) {
    for node in nodes {
        out.push(Section {
            id: node.get_str("forum_id").unwrap_or_default().to_owned(),
            name: node.get_str("forum_name").unwrap_or_default().to_owned(),
            depth,
            sub_only: node
                .get("sub_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        if let Some(children) = node.get("child").and_then(Value::as_array) {
            flatten(children, depth + 1, out);
        }
    }
}

pub async fn topics(fetch: &Fetch, forum: &Forum, forum_id: &str, page: i64) -> Result<TopicList> {
    let (start, end) = span(page);
    let value = call(
        fetch,
        forum,
        "get_topic",
        &[
            Arg::Str(forum_id.to_owned()),
            Arg::Int(start),
            Arg::Int(end),
        ],
    )
    .await?;
    Ok(topic_list(&value))
}

pub async fn search(fetch: &Fetch, forum: &Forum, query: &str, page: i64) -> Result<TopicList> {
    let per = limits::get().forums.page_size.value;
    let value = call(
        fetch,
        forum,
        "search",
        &[Arg::Struct(vec![
            ("keywords", Arg::b64(query)),
            ("page", Arg::Int(page.max(1))),
            ("perpage", Arg::Int(per)),
        ])],
    )
    .await?;
    Ok(topic_list(&value))
}

fn topic_list(value: &Value) -> TopicList {
    let topics = value
        .get("topics")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|t| Topic {
            id: t.get_str("topic_id").unwrap_or_default().to_owned(),
            title: t.get_str("topic_title").unwrap_or_default().to_owned(),
            author: t
                .get_str("topic_author_name")
                .unwrap_or_default()
                .to_owned(),
            replies: t.get("reply_number").and_then(Value::as_i64).unwrap_or(0),
        })
        .collect();
    TopicList {
        topics,
        total: value.get("total_topic_num").and_then(Value::as_i64),
    }
}

/// How many posts a thread has, with a one-post fetch, so a caller can jump to the last page.
pub async fn thread_total(fetch: &Fetch, forum: &Forum, topic_id: &str) -> Result<i64> {
    let value = call(
        fetch,
        forum,
        "get_thread",
        &[
            Arg::Str(topic_id.to_owned()),
            Arg::Int(0),
            Arg::Int(0),
            Arg::Bool(false),
        ],
    )
    .await?;
    Ok(value
        .get("total_post_num")
        .and_then(Value::as_i64)
        .unwrap_or(0))
}

pub async fn thread(fetch: &Fetch, forum: &Forum, topic_id: &str, page: i64) -> Result<Thread> {
    let (start, end) = span(page);
    let value = call(
        fetch,
        forum,
        "get_thread",
        &[
            Arg::Str(topic_id.to_owned()),
            Arg::Int(start),
            Arg::Int(end),
            Arg::Bool(false),
        ],
    )
    .await?;
    let posts = value
        .get("posts")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|p| Post {
            id: p.get_str("post_id").unwrap_or_default().to_owned(),
            author: p.get_str("post_author_name").unwrap_or_default().to_owned(),
            time: p.get_str("post_time").unwrap_or_default().to_owned(),
            content: p.get_str("post_content").unwrap_or_default().to_owned(),
        })
        .collect();
    Ok(Thread {
        title: value.get_str("topic_title").unwrap_or_default().to_owned(),
        total: value.get("total_post_num").and_then(Value::as_i64),
        posts,
    })
}

pub async fn login(fetch: &Fetch, forum: &Forum, user: &str, password: &str) -> Result<Login> {
    let (value, set) = send(fetch, forum, "login", &[Arg::b64(user), Arg::b64(password)]).await?;
    if value.get("result").and_then(Value::as_bool) == Some(false) {
        bail!("{}", value.get_str("result_text").unwrap_or("login failed"));
    }
    let cookies = if set.is_empty() {
        forum.cookies.clone()
    } else {
        set
    };
    Ok(Login {
        username: value.get_str("username").unwrap_or(user).to_owned(),
        cookies,
    })
}

pub async fn reply(
    fetch: &Fetch,
    forum: &Forum,
    forum_id: &str,
    topic_id: &str,
    body: &str,
) -> Result<Posted> {
    let (value, _) = send(
        fetch,
        forum,
        "reply_post",
        &[
            Arg::Str(forum_id.to_owned()),
            Arg::Str(topic_id.to_owned()),
            Arg::b64(""),
            Arg::b64(body),
        ],
    )
    .await?;
    posted(value)
}

pub async fn new_topic(
    fetch: &Fetch,
    forum: &Forum,
    forum_id: &str,
    subject: &str,
    body: &str,
) -> Result<Posted> {
    let (value, _) = send(
        fetch,
        forum,
        "new_topic",
        &[
            Arg::Str(forum_id.to_owned()),
            Arg::b64(subject),
            Arg::b64(body),
        ],
    )
    .await?;
    posted(value)
}

fn posted(value: Value) -> Result<Posted> {
    if value.get("result").and_then(Value::as_bool) == Some(false) {
        bail!(
            "{}",
            value
                .get_str("result_text")
                .unwrap_or("the forum rejected the post")
        );
    }
    Ok(Posted {
        id: value.get_str("post_id").map(str::to_owned),
        url: value
            .get_str("post_url")
            .or_else(|| value.get_str("topic_url"))
            .map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use axum::Router;
    use axum::body::Bytes;
    use axum::http::HeaderMap;
    use axum::response::Response;
    use axum::routing::post;

    use super::*;

    /// A fake mobiquo endpoint: it records the last call's method, cookie and body, and answers from
    /// the method name. One Set-Cookie on login lets the cookie-replay path be checked.
    #[derive(Default)]
    struct Seen {
        cookie: String,
        body: String,
    }

    fn b64(text: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    async fn fake() -> (Forum, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen_in = seen.clone();

        async fn handle(
            seen: axum::extract::State<Arc<Mutex<Seen>>>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            let text = String::from_utf8_lossy(&body).into_owned();
            {
                let mut s = seen.lock().unwrap();
                s.cookie = headers
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                s.body = text.clone();
            }
            let method = text
                .split("<methodName>")
                .nth(1)
                .and_then(|m| m.split("</methodName>").next())
                .unwrap_or("");
            let xml: String = match method {
                "get_forum" => format!(
                    "<methodResponse><params><param><value><array><data>\
                     <value><struct>\
                       <member><name>forum_id</name><value><string>1</string></value></member>\
                       <member><name>forum_name</name><value><base64>{}</base64></value></member>\
                       <member><name>sub_only</name><value><boolean>1</boolean></value></member>\
                       <member><name>child</name><value><array><data>\
                         <value><struct>\
                           <member><name>forum_id</name><value><string>7</string></value></member>\
                           <member><name>forum_name</name><value><base64>{}</base64></value></member>\
                         </struct></value>\
                       </data></array></value></member>\
                     </struct></value>\
                     </data></array></value></param></params></methodResponse>",
                    b64("Games"),
                    b64("Counter-Strike"),
                ),
                "get_thread" => format!(
                    "<methodResponse><params><param><value><struct>\
                       <member><name>topic_title</name><value><base64>{}</base64></value></member>\
                       <member><name>total_post_num</name><value><int>1</int></value></member>\
                       <member><name>posts</name><value><array><data>\
                         <value><struct>\
                           <member><name>post_id</name><value><string>9</string></value></member>\
                           <member><name>post_author_name</name><value><base64>{}</base64></value></member>\
                           <member><name>post_content</name><value><base64>{}</base64></value>\
                         </struct></value>\
                       </data></array></value></member>\
                     </struct></value></param></params></methodResponse>",
                    b64("Ümlaut topic"),
                    b64("crow"),
                    b64("hello & <welcome>"),
                ),
                "search" => {
                    // Guests get the refusal; a session (cookie present) gets a hit.
                    let logged_in = seen.lock().unwrap().cookie.contains("bbsessionhash");
                    if logged_in {
                        format!(
                            "<methodResponse><params><param><value><struct>\
                               <member><name>total_topic_num</name><value><int>1</int></value></member>\
                               <member><name>topics</name><value><array><data>\
                                 <value><struct>\
                                   <member><name>topic_id</name><value><string>42</string></value></member>\
                                   <member><name>topic_title</name><value><base64>{}</base64></value></member>\
                                 </struct></value>\
                               </data></array></value></member>\
                             </struct></value></param></params></methodResponse>",
                            b64("found it"),
                        )
                    } else {
                        format!(
                            "<methodResponse><params><param><value><struct>\
                               <member><name>result</name><value><boolean>0</boolean></value></member>\
                               <member><name>result_text</name><value><base64>{}</base64></value>\
                             </struct></value></param></params></methodResponse>",
                            b64("You are not logged in"),
                        )
                    }
                }
                "login" => format!(
                    "<methodResponse><params><param><value><struct>\
                       <member><name>result</name><value><boolean>1</boolean></value></member>\
                       <member><name>user_id</name><value><string>5</string></value></member>\
                       <member><name>username</name><value><base64>{}</base64></value>\
                     </struct></value></param></params></methodResponse>",
                    b64("crow"),
                ),
                _ => unreachable!("fake got {method}"),
            };
            Response::builder()
                .header("set-cookie", "bbsessionhash=sess123; path=/; HttpOnly")
                .body(axum::body::Body::from(xml))
                .unwrap()
        }

        let router = Router::new()
            .route("/mob/mobiquo.php", post(handle))
            .with_state(seen_in);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let forum = Forum {
            id: "1".into(),
            name: "Fake".into(),
            base_url: base,
            mobiquo_dir: "mob".into(),
            ext: "php".into(),
            kind: "mobiquo".into(),
            ..Forum::default()
        };
        (forum, seen)
    }

    fn client() -> Fetch {
        Fetch::new(Duration::from_secs(5)).unwrap()
    }

    #[tokio::test]
    async fn sections_flatten_the_tree_with_depth() {
        let (forum, _) = fake().await;
        let got = sections(&client(), &forum).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Games");
        assert!(got[0].sub_only);
        assert_eq!(got[1].depth, 1);
        assert_eq!(got[1].name, "Counter-Strike");
    }

    #[tokio::test]
    async fn a_thread_decodes_base64_and_entities_and_sends_the_right_call() {
        let (forum, seen) = fake().await;
        let t = thread(&client(), &forum, "77", 1).await.unwrap();
        assert_eq!(t.title, "Ümlaut topic");
        assert_eq!(t.posts[0].author, "crow");
        assert_eq!(t.posts[0].content, "hello & <welcome>");
        let body = &seen.lock().unwrap().body;
        assert!(
            body.contains("<methodName>get_thread</methodName>"),
            "{body}"
        );
        // page 1 of size 20 is the inclusive range 0..=19.
        assert!(
            body.contains("<int>0</int>") && body.contains("<int>19</int>"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn search_refuses_a_guest_and_serves_a_session() {
        let (mut forum, _) = fake().await;
        let guest = search(&client(), &forum, "aimbot", 1).await.unwrap_err();
        assert!(guest.to_string().contains("not logged in"), "{guest}");

        forum.cookies = vec!["bbsessionhash=sess123; path=/".into()];
        let hit = search(&client(), &forum, "aimbot", 1).await.unwrap();
        assert_eq!(hit.topics[0].title, "found it");
    }

    #[tokio::test]
    async fn login_names_the_user_and_keeps_the_session_cookie() {
        let (forum, seen) = fake().await;
        let who = login(&client(), &forum, "crow", "secret").await.unwrap();
        assert_eq!(who.username, "crow");
        assert_eq!(who.cookies, ["bbsessionhash=sess123; path=/; HttpOnly"]);
        // The password went up base64-wrapped, never in the clear.
        let body = &seen.lock().unwrap().body;
        assert!(
            body.contains(&format!("<base64>{}</base64>", b64("secret"))),
            "{body}"
        );
    }

    #[test]
    fn a_cookie_header_takes_each_name_value_and_the_endpoint_uses_the_renamed_dir() {
        let header = cookie_header(&[
            "bbsessionhash=sess123; path=/; HttpOnly".into(),
            "x=1".into(),
        ]);
        assert_eq!(header.as_deref(), Some("bbsessionhash=sess123; x=1"));
        let forum = Forum {
            base_url: "https://f.example/forum/".into(),
            mobiquo_dir: "abc123".into(),
            ext: "php".into(),
            ..Forum::default()
        };
        assert_eq!(
            endpoint(&forum),
            "https://f.example/forum/abc123/mobiquo.php"
        );
    }
}
