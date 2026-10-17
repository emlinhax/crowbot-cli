//! MCP servers in a session: what an interrupt tells them, and the question about servers
//! other apps list.

use super::harness::{CTRL_D, ESC, Session, in_session_with};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_mcp_call_is_cancelled_at_the_server() {
    let fake = Fake::start().await;
    fake.script([Reply::call("fake__slow", "{}")]);
    let sandbox = Sandbox::default();
    let server = env!("CARGO_BIN_EXE_crowbot-fake-mcp").replace('\\', "/");
    sandbox.write_home(
        "config.toml",
        &format!("[mcp.fake]\ncommand = '{server}'\nargs = ['--log', 'heard.txt']\n"),
    );
    let ended = in_session_with(&fake, sandbox, Some(ENV_KEY), &["--mode", "auto"], |s| {
        s.submit("wait for it");
        s.wait_for("fake__slow");
        s.send(ESC);
        s.wait_for("Interrupted.");
    })
    .await;
    let heard = std::fs::read_to_string(ended.sandbox.project().join("heard.txt")).unwrap();
    assert!(
        heard.ends_with("tools/call slow\nnotifications/cancelled 3\n"),
        "{heard}\n{}",
        ended.screen
    );
}

/// `{"mcpServers": {"fake": …}}` as Cursor writes it, starting the stand-in server with `args`.
fn cursor_lists_fake(sandbox: &Sandbox, args: &[&str]) {
    let server = env!("CARGO_BIN_EXE_crowbot-fake-mcp").replace('\\', "/");
    let entry = serde_json::json!({"mcpServers": {"fake": {"command": server, "args": args}}});
    sandbox.write_user(".cursor/mcp.json", &entry.to_string());
}

/// Starts a session, answers the question about found servers with `answer` (or waits for none
/// to come when `None`), and quits; the screen as it was.
fn start(
    sandbox: &Sandbox,
    url: &str,
    answer: Option<&str>,
    then: impl FnOnce(&mut Session),
) -> String {
    let mut s = Session::start_as(sandbox, url, Some(ENV_KEY), &[]);
    if let Some(answer) = answer {
        s.wait_for("Use them in crowbot?");
        s.send(answer);
    }
    s.wait_bar(" · ");
    then(&mut s);
    let screen = s.contents();
    s.send(CTRL_D);
    assert!(s.wait_exit(), "{}", s.contents());
    screen
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_another_app_lists_is_asked_about_once_and_then_used() {
    let fake = Fake::start().await;
    fake.script([Reply::text("Hello.")]);
    let sandbox = Sandbox::default();
    cursor_lists_fake(&sandbox, &[]);
    let url = fake.url.clone();
    let (later, sandbox) = tokio::task::spawn_blocking(move || {
        let status = |s: &mut Session| {
            s.submit("/status");
            s.wait_for("MCP: fake (Cursor), 7 tools");
        };
        start(&sandbox, &url, Some("y\r"), status);
        let later = start(&sandbox, &url, None, status);
        (later, sandbox)
    })
    .await
    .unwrap();
    assert!(!later.contains("Use them in crowbot?"), "{later}");
    // A run that cannot ask uses what was chosen.
    sandbox
        .run(&fake.url, &["-p", "hi"], Some(ENV_KEY))
        .await
        .success();
    let tools = fake.chat_bodies().last().unwrap()["tools"].to_string();
    assert!(tools.contains("fake__echo"), "{tools}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_server_is_not_asked_about_again_until_what_it_runs_changes() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    cursor_lists_fake(&sandbox, &[]);
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let none = |s: &mut Session| {
            s.submit("/status");
            s.wait_for("MCP: no servers");
        };
        start(&sandbox, &url, Some("n\r"), none);
        let later = start(&sandbox, &url, None, none);
        assert!(!later.contains("Use them in crowbot?"), "{later}");
        cursor_lists_fake(&sandbox, &["--log", "heard.txt"]);
        start(&sandbox, &url, Some("n\r"), |_| {});
    })
    .await
    .unwrap();
}
