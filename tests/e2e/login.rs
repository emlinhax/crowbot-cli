use crate::Sandbox;
use crate::fake_crowbot::{Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn pairing_saves_a_device_key_that_chat_then_uses() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["login"], &[]).await;
    let out = run.success().stdout();
    assert!(out.contains("ABCD-1234"), "{out}");
    assert!(out.contains("/pair"), "{out}");
    assert!(out.contains("Paired"), "{out}");
    assert_eq!(fake.hits("/api/pair/dev1"), 2);

    let stored = std::fs::read_to_string(sandbox.home().join("auth.json")).unwrap();
    assert!(stored.contains("\"device\""));
    let status = sandbox.run(&fake.url, &["login", "--status"], &[]).await;
    assert!(status.success().stdout().contains("device key …9999"));

    fake.script([Reply::Sse("hello.sse")]);
    let chat = sandbox.run(&fake.url, &["-p", "hi"], &[]).await;
    assert_eq!(chat.success().stdout(), "Hello there!\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_number_is_checked_before_it_is_saved() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let bad = sandbox
        .run(&fake.url, &["login", "--key", "0000 0000 0000 0000"], &[])
        .await;
    assert_eq!(bad.code(), Some(1));
    assert!(!sandbox.home().join("auth.json").exists());

    let good = sandbox
        .run(&fake.url, &["login", "--key", "1234 5678 9012 3456"], &[])
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
        .run(&fake.url, &["login", "--key", "1234567890123456"], &[])
        .await
        .success();

    let out = sandbox.run(&fake.url, &["logout"], &[]).await;
    assert!(out.success().stdout().contains("Logged out"));
    assert!(!sandbox.home().join("auth.json").exists());
    let again = sandbox.run(&fake.url, &["logout"], &[]).await;
    assert!(again.success().stdout().contains("No key"));
}
