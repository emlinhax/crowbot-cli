//! `/login` and `/signup` in a session, what the account's state shows, and a command typed
//! while a turn runs.

use super::harness::{ENTER, ESC, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ACCOUNT_NUMBER, ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_command_typed_during_a_run_never_reaches_the_model() {
    let fake = Fake::start().await;
    fake.script([
        Reply::call("bash", r#"{"command": "sleep 3"}"#),
        Reply::sse("hello.sse"),
    ]);
    let sandbox = Sandbox::default();
    sandbox.write_home("config.toml", "mode = \"auto\"\n");
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.submit("go");
        s.wait_for("sleep 3");
        s.submit("/login --key 1234 5678 9012 3456");
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
        s.submit("/login");
        s.wait_for("Log in to crowbot");
        s.send("1");
        s.wait_for("ABCD-1234");
        s.wait_for("Logged in with device key …9999");
        // The key is live in this session: no restart needed.
        s.submit("hi");
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
        s.submit("/login");
        s.wait_for("Log in to crowbot");
        s.send("2");
        s.submit("0000 0000 0000 0000");
        s.wait_for("crowbot does not know that key");
        s.send("2");
        s.type_text("1234 5678 9012 3456");
        s.wait_for("•••• •••• •••• 3456");
        assert!(!s.contents().contains("1234 5678"), "{}", s.contents());
        s.send(ENTER);
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
        s.submit("/login --status");
        s.wait_for("Not logged in. Run /login");
        s.submit("/login --key 1234 5678 9012 3456");
        s.wait_for("Logged in with account number …3456");
    })
    .await;
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bar_shows_the_balance_and_follows_login_and_logout() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        assert!(!s.bottom_row().contains("left"), "{}", s.bottom_row());
        s.submit("/login --key 1234 5678 9012 3456");
        s.wait_bar("$12.30 left");
        s.submit("/logout");
        s.wait_for("Logged out");
        s.wait_bar_gone("$12.30 left");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_cannot_spend_says_so_at_the_start() {
    let fake = Fake::start().await;
    fake.set_me(serde_json::json!({"balance_microdollars": 0, "can_spend": false}));
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("The account cannot spend yet");
        s.wait_bar("$0.00 left");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_key_file_is_named_in_the_welcome() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.write_home("auth.json", "not json");
    in_session(&fake, sandbox, None, |s| {
        s.wait_for("auth.json is unreadable");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_asks_before_replacing_a_stored_account_number() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", ACCOUNT_NUMBER], None)
        .await
        .success();
    let ended = in_session(&fake, sandbox, None, |s| {
        s.submit("/login");
        s.wait_for("Replace the stored key?");
        assert!(
            s.contents().contains("account number …3456"),
            "{}",
            s.contents()
        );
        s.send("2");
        s.wait_gone("Replace the stored key?");
        s.submit("/login");
        s.wait_for("Replace the stored key?");
        s.send("1");
        s.wait_for("Pair this device");
        s.send("1");
        s.wait_for("Logged in with device key …9999");
    })
    .await;
    let stored = std::fs::read_to_string(ended.sandbox.home().join("auth.json")).unwrap();
    assert!(stored.contains("\"device\""), "{stored}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_key_mid_session_offers_the_login_card() {
    let fake = Fake::start().await;
    fake.script([Reply::Error {
        status: 401,
        kind: "invalid_api_key".into(),
        retry_after: None,
    }]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Your key was not accepted");
        s.wait_for("Log in to crowbot");
        s.wait_for("Pair this device");
        s.send(ESC);
        s.wait_gone("Log in to crowbot");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_funds_and_a_spent_allowance_say_where_the_account_stands() {
    let fake = Fake::start().await;
    let refused = |kind: &str| Reply::Error {
        status: 402,
        kind: kind.into(),
        retry_after: None,
    };
    fake.script([
        refused("insufficient_balance"),
        refused("plan_allowance_spent"),
    ]);
    fake.set_me(
        serde_json::json!({"balance_microdollars": 0, "can_spend": false,
        "pass": {"label": "Pro", "allowance_bp": 0, "refills": "5 hours"}}),
    );
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Out of funds");
        s.wait_for("Balance $0.00. Plan Pro: 0% of its allowance left, full in 5 hours.");
        s.wait_bar("Pro 0%");
        s.submit("again");
        s.wait_for("allowance is used up");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_signup_creates_an_account_in_a_card_and_the_session_uses_it() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("/signup for a new account");
        s.submit("/signup");
        s.wait_for("Your account number:  1234 5678 9012 3456");
        s.submit("0000");
        s.wait_for("does not match");
        s.submit("3456");
        s.wait_for("Account …3456 created");
        s.wait_bar("$12.30 left");
        s.submit("hi");
        s.wait_for("Hello there!");
    })
    .await;
    assert!(ended.sandbox.home().join("auth.json").exists());
    assert!(!ended.screen.contains("1234 5678"), "{}", ended.screen);
    let sessions: String = ended
        .sandbox
        .sessions()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    assert!(!sessions.contains("9012"), "the number was saved: {sessions}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_signup_asks_before_replacing_a_stored_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", ACCOUNT_NUMBER], None)
        .await
        .success();
    in_session(&fake, sandbox, None, |s| {
        s.submit("/signup");
        s.wait_for("Replace the stored key?");
        s.send("2");
        s.wait_gone("Replace the stored key?");
    })
    .await;
    assert_eq!(fake.hits("/api/pow"), 0);
}
