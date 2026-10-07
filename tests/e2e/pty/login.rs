//! `/login` in a session, and a command typed while a turn runs.

use super::harness::in_session;
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_command_typed_during_a_run_never_reaches_the_model() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("pty/sleep.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home.path()).unwrap();
    std::fs::write(sandbox.home.path().join("config.toml"), "mode = \"auto\"\n").unwrap();
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("go");
        s.send("\r");
        s.wait_for("sleep 3");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
        s.wait_for("Hello there!");
        s.contents()
    })
    .await;
    // The command ran mid-turn, echoed with its number hidden.
    assert!(ended.out.contains("/login --key …3456"), "{}", ended.out);
    let sessions: String = ended
        .sandbox
        .sessions()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let bodies = serde_json::to_string(&fake.chat_bodies()).unwrap();
    assert_eq!(fake.hits("/v1/chat/completions"), 2);
    for secret in ["9012", "3456"] {
        assert!(
            !bodies.contains(secret),
            "{secret} reached the model: {bodies}"
        );
        assert!(!sessions.contains(secret), "{secret} was saved: {sessions}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_pairs_this_device_and_the_session_chats_at_once() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to crowbot");
        s.send("1");
        s.wait_for("ABCD-1234");
        s.wait_for("Logged in with device key …9999");
        // The key is live in this session: no restart needed.
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert!(ended.screen.contains("$12.30"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_a_masked_account_number_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to crowbot");
        s.send("2");
        s.type_text("0000 0000 0000 0000");
        s.send("\r");
        s.wait_for("invalid_api_key");
        s.send("2");
        s.type_text("1234 5678 9012 3456");
        s.wait_for("•••• •••• •••• 3456");
        assert!(!s.contents().contains("1234 5678"), "{}", s.contents());
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
    })
    .await;
    assert!(!ended.screen.contains("1234 5678"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_the_same_arguments_as_the_command_line() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login --status");
        s.send("\r");
        s.wait_for("Not logged in. Run /login");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
    })
    .await;
    assert!(ended.sandbox.home().join("auth.json").exists());
}
