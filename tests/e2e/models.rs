use crate::fake_crowbot::Fake;
use crate::{DEAD_URL, Sandbox};

#[tokio::test(flavor = "multi_thread")]
async fn lists_live_models_then_serves_them_from_cache() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let first = sandbox.run(&fake.url, &["models"], &[]).await;
    let out = first.success().stdout();
    assert!(out.contains("fake-coder"), "{out}");
    assert!(out.contains("Live list"), "{out}");
    assert!(sandbox.home().join("cache/models.json").exists());

    let second = sandbox.run(&fake.url, &["models"], &[]).await;
    assert!(second.success().stdout().contains("Cached list"));
    assert_eq!(fake.hits("/v1/models"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_bypasses_a_fresh_cache() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["models"], &[]).await.success();
    let out = sandbox.run(&fake.url, &["models", "--refresh"], &[]).await;
    assert!(out.success().stdout().contains("Live list"));
    assert_eq!(fake.hits("/v1/models"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_falls_back_to_the_bundled_list() {
    let sandbox = Sandbox::default();
    let run = sandbox.run(DEAD_URL, &["models"], &[]).await;
    let out = run.success().stdout();
    assert!(out.contains("Offline"), "{out}");
    assert!(out.contains("crow-2"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_prefers_a_stale_cache_over_the_bundled_list() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["models"], &[]).await.success();
    let out = sandbox.run(DEAD_URL, &["models", "--refresh"], &[]).await;
    let out = out.success().stdout();
    assert!(out.contains("fake-coder"), "{out}");
    assert!(out.contains("Could not reach crowbot"), "{out}");
}
