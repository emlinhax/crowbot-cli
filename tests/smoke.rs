//! Against the real api.crowbot.sh; spends real (tiny) money. Run with `make smoke`,
//! which needs CROWBOT_SMOKE_KEY. Never part of CI.
#![allow(clippy::disallowed_methods)]

#[path = "common/mod.rs"]
mod common;

use common::Run;

/// The cheapest live model; a smoke run costs a small fraction of a cent.
const MODEL: &str = "deepseek-v4.1-flash";
/// Upper bound on what one smoke request may cost, in micro-dollars.
const MAX_COST_MICROS: u64 = 1000;

/// Runs crowbot against the live API, launched as e2e launches it.
async fn crowbot(args: &[&str]) -> Run {
    let key = std::env::var("CROWBOT_SMOKE_KEY").expect("set CROWBOT_SMOKE_KEY");
    let (home, user) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // An empty project: the live model sees neither this checkout nor its .crowbot config.
    let project = tempfile::tempdir().unwrap();
    common::launch(
        home.path(),
        user.path(),
        project.path(),
        args,
        None,
        Some(&key),
    )
    .await
}

#[tokio::test]
#[ignore = "spends real money; run with `make smoke`"]
async fn live_models_include_the_smoke_model() {
    let out = crowbot(&["models", "--refresh"]).await;
    out.success().stdout_has("Live list").stdout_has(MODEL);
}

#[tokio::test]
#[ignore = "spends real money; run with `make smoke`"]
async fn a_real_round_trip_is_cheap_and_well_formed() {
    let out = crowbot(&[
        "--model",
        MODEL,
        "--json",
        "Reply with exactly the word PONG and nothing else.",
    ])
    .await;
    let end = out
        .success()
        .events()
        .into_iter()
        .find(|e| e["type"] == "message_end")
        .expect("a message_end event");
    let message = &end["message"];
    let text: String = message["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["type"] == "text")
        .filter_map(|p| p["text"].as_str())
        .collect();
    assert!(text.contains("PONG"), "{text}");
    assert_eq!(message["finish"], "done");
    assert!(message["request_id"].is_string(), "{message}");
    let cost = message["usage"]["cost_micros"].as_u64().unwrap();
    assert!(cost > 0 && cost < MAX_COST_MICROS, "cost {cost}");
}
