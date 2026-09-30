use crate::fake_crowbot::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn signup_solves_the_challenge_and_saves_the_account() {
    let fake = Fake::start().await;
    fake.stale_proofs(1);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["signup"], None).await;
    let out = run.success().stdout();
    assert!(out.contains("1234 5678 9012 3456"), "{out}");
    assert!(out.contains("no recovery"), "{out}");
    // The first proof was refused as stale, so a fresh challenge was fetched.
    assert_eq!(fake.hits("/api/pow"), 2);

    fake.script([Reply::sse("hello.sse")]);
    let chat = sandbox.run(&fake.url, &["-p", "hi"], None).await;
    assert_eq!(chat.success().stdout(), "Hello there!\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_key_neither_blocks_signup_nor_hides_what_it_shadows() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let note = "CROWBOT_API_KEY is set in this environment";

    let signup = sandbox.run(&fake.url, &["signup"], WITH_KEY).await;
    assert!(
        signup.success().stdout().contains(note),
        "{}",
        signup.stdout()
    );
    assert!(sandbox.home().join("auth.json").exists());

    let args = ["login", "--key", "1234 5678 9012 3456"];
    let login = sandbox.run(&fake.url, &args, WITH_KEY).await;
    assert!(
        login.success().stdout().contains(note),
        "{}",
        login.stdout()
    );

    let logout = sandbox.run(&fake.url, &["logout"], WITH_KEY).await;
    assert!(
        logout.success().stdout().contains(note),
        "{}",
        logout.stdout()
    );
    let quiet = sandbox.run(&fake.url, &["logout"], None).await;
    assert!(!quiet.success().stdout().contains(note));
}

#[tokio::test(flavor = "multi_thread")]
async fn signup_refuses_to_replace_an_existing_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox
        .run(&fake.url, &["login", "--key", "1234567890123456"], None)
        .await
        .success();

    let run = sandbox.run(&fake.url, &["signup"], None).await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("already logged in"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/api/pow"), 0);
}
