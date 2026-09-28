//! Against the real api.crowbot.sh; spends real (tiny) money. Run with `make smoke`,
//! which needs CROWBOT_SMOKE_KEY. Never part of CI.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::process::{Command, Output, Stdio};

/// The cheapest live model; a smoke run costs a small fraction of a cent.
const MODEL: &str = "deepseek-v4.1-flash";
/// Upper bound on what one smoke request may cost, in micro-dollars.
const MAX_COST_MICROS: u64 = 1000;

fn crowbot(args: &[&str]) -> Output {
    let key = std::env::var("CROWBOT_SMOKE_KEY").expect("set CROWBOT_SMOKE_KEY");
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_crowbot"))
        .args(args)
        .env("CROWBOT_HOME", home.path())
        .env("CROWBOT_API_KEY", key)
        .env_remove("CROWBOT_API_URL")
        .env_remove("CROWBOT_CHAT_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
#[ignore = "spends real money; run with `make smoke`"]
fn live_models_include_the_smoke_model() {
    let out = crowbot(&["models", "--refresh"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Live list"), "{text}");
    assert!(text.contains(MODEL), "{text}");
}

#[test]
#[ignore = "spends real money; run with `make smoke`"]
fn a_real_round_trip_is_cheap_and_well_formed() {
    let out = crowbot(&[
        "--model",
        MODEL,
        "--json",
        "Reply with exactly the word PONG and nothing else.",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let end: serde_json::Value = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
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
