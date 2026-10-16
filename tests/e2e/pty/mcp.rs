//! MCP servers in a session: what an interrupt tells them.

use super::harness::{ESC, in_session_with};
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
