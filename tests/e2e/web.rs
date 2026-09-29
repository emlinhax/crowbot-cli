//! webfetch through the real binary and the real cffetch client, against pages the fake serves:
//! a readable one, and one that answers the way Cloudflare does while it challenges a client.

use crate::fake_crowbot::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

/// What crowbot told the model its webfetch call returned.
fn tool_result(fake: &Fake) -> String {
    let bodies = fake.chat_bodies();
    let messages = bodies[1]["messages"].as_array().expect("a second request");
    let tool = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the tool result goes back to the model");
    tool["content"].as_str().unwrap_or_default().to_owned()
}

async fn fetch(fixture: &str) -> (Fake, String) {
    let fake = Fake::start().await;
    fake.script([Reply::sse(fixture), Reply::sse("web/done.sse")]);
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "-p", "read the docs"],
            WITH_KEY,
        )
        .await;
    assert_eq!(run.success().stdout(), "Done.\n");
    let result = tool_result(&fake);
    (fake, result)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_reaches_the_model_as_markdown() {
    let (fake, result) = fetch("web/fetch_ok.sse").await;
    assert!(result.contains("# Widget API"), "{result}");
    assert!(result.contains("The widget spins at 3 rpm."), "{result}");
    assert_eq!(fake.hits("/web/ok"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_challenge_is_reported_never_passed_off_as_the_page() {
    let (fake, result) = fetch("web/fetch_blocked.sse").await;
    assert!(result.contains("Cloudflare challenge"), "{result}");
    assert!(result.contains("do not retry"), "{result}");
    assert!(!result.contains("Just a moment"), "{result}");
    // A challenged profile is followed by another before giving up.
    assert!(
        fake.hits("/web/blocked") > 1,
        "{}",
        fake.hits("/web/blocked")
    );
}
