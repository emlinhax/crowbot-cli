use crate::Sandbox;
use crate::fake_crowbot::{Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn signup_solves_the_challenge_and_saves_the_account() {
    let fake = Fake::start().await;
    fake.stale_proofs(1);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["signup"], &[]).await;
    let out = run.success().stdout();
    assert!(out.contains("1234 5678 9012 3456"), "{out}");
    assert!(out.contains("no recovery"), "{out}");
    // The first proof was refused as stale, so a fresh challenge was fetched.
    assert_eq!(fake.hits("/api/pow"), 2);

    fake.script([Reply::Sse("hello.sse")]);
    let chat = sandbox.run(&fake.url, &["-p", "hi"], &[]).await;
    assert_eq!(chat.success().stdout(), "Hello there!\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn signup_refuses_to_replace_an_existing_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", "1234567890123456"], &[])
        .await
        .success();

    let run = sandbox.run(&fake.url, &["signup"], &[]).await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("already logged in"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/api/pow"), 0);
}
