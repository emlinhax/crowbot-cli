use crate::Sandbox;
use crate::fake_crowbot::{Fake, PairPoll, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn pairing_saves_a_device_key_that_chat_then_uses() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["login"], None).await;
    let out = run.success().stdout();
    assert!(out.contains("ABCD-1234"), "{out}");
    assert!(out.contains("/pair"), "{out}");
    assert!(out.contains("Logged in with device key …9999"), "{out}");
    assert!(out.contains("$12.30"), "{out}");
    assert_eq!(fake.hits("/api/pair/dev1"), 2);

    let stored = std::fs::read_to_string(sandbox.home().join("auth.json")).unwrap();
    assert!(stored.contains("\"device\""));
    let status = sandbox.run(&fake.url, &["login", "--status"], None).await;
    assert!(status.success().stdout().contains("device key …9999"));

    fake.script([Reply::sse("hello.sse")]);
    let chat = sandbox.run(&fake.url, &["-p", "hi"], None).await;
    assert_eq!(chat.success().stdout(), "Hello there!\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_waits_out_a_passing_error_and_stops_at_a_final_one() {
    let fake = Fake::start().await;
    let blip = PairPoll::Error {
        status: 503,
        kind: "upstream_unavailable",
    };
    fake.pair_polls([blip]);
    let sandbox = Sandbox::default();
    let run = sandbox.run(&fake.url, &["login"], None).await;
    assert!(run.success().stdout().contains("Logged in with device key"));
    assert_eq!(fake.hits("/api/pair/dev1"), 2);

    let fake = Fake::start().await;
    fake.pair_polls([PairPoll::Error {
        status: 401,
        kind: "invalid_api_key",
    }]);
    let run = sandbox.run(&fake.url, &["login"], None).await;
    assert_eq!(run.code(), Some(1));
    assert_eq!(fake.hits("/api/pair/dev1"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_number_is_checked_before_it_is_saved() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let bad = sandbox
        .run(&fake.url, &["login", "--key", "0000 0000 0000 0000"], None)
        .await;
    assert_eq!(bad.code(), Some(1));
    assert!(!sandbox.home().join("auth.json").exists());

    let good = sandbox
        .run(&fake.url, &["login", "--key", "1234 5678 9012 3456"], None)
        .await;
    let out = good.success().stdout();
    assert!(out.contains("…3456"), "{out}");
    assert!(out.contains("$12.30"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_removes_the_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", "1234567890123456"], None)
        .await
        .success();

    let out = sandbox.run(&fake.url, &["logout"], None).await;
    assert!(out.success().stdout().contains("Logged out"));
    assert!(!sandbox.home().join("auth.json").exists());
    let again = sandbox.run(&fake.url, &["logout"], None).await;
    assert!(again.success().stdout().contains("No key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_key_file_is_explained_and_logout_still_clears_it() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home()).unwrap();
    std::fs::write(sandbox.home().join("auth.json"), "not json").unwrap();

    let status = sandbox.run(&fake.url, &["login", "--status"], None).await;
    assert_ne!(status.code(), Some(0));
    assert!(status.stderr().contains("auth.json"), "{}", status.stderr());
    assert!(
        status.stderr().contains("crowbot logout"),
        "{}",
        status.stderr()
    );

    sandbox.run(&fake.url, &["logout"], None).await.success();
    assert!(!sandbox.home().join("auth.json").exists());
    let status = sandbox.run(&fake.url, &["login", "--status"], None).await;
    assert!(status.success().stdout().contains("Not logged in"));
}
