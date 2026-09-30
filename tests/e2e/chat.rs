use serde_json::Value;

use crate::fake_crowbot::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn print_streams_the_reply_and_records_the_session() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox
        .run(&fake.url, &["-p", "hi", "there"], WITH_KEY)
        .await;
    assert_eq!(run.success().stdout(), "Hello there!\n");

    let body = &fake.chat_bodies()[0];
    assert_eq!(body["model"], "crow-2");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"], "hi there");

    let sessions = sandbox.sessions();
    assert_eq!(sessions.len(), 1);
    let lines: Vec<Value> = std::fs::read_to_string(&sessions[0])
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["type"], "session");
    let reply = &lines[2]["message"];
    assert_eq!(reply["role"], "assistant");
    assert_eq!(reply["parts"][0]["type"], "reasoning");
    assert_eq!(reply["request_id"], "req_fake");
    // 600 uncached * $6 + 400 cached * $0.60 + 100 out * $10, in micro-dollars
    assert_eq!(reply["usage"]["cost_micros"], 4840);
}

#[tokio::test(flavor = "multi_thread")]
async fn json_mode_emits_one_event_per_line() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["--json", "hi"], WITH_KEY).await;
    let stdout = run.success().stdout();
    for line in stdout.lines() {
        serde_json::from_str::<Value>(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    }
    // The stream is a contract for scripts: its golden changes only on purpose.
    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/json/hello.jsonl");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &stdout).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(stdout, want, "--json changed; if on purpose, `make golden`");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limits_are_retried_before_anything_streams() {
    let fake = Fake::start().await;
    fake.script([
        Reply::Error {
            status: 429,
            kind: "rate_limited",
            retry_after: Some(0),
        },
        Reply::sse("hello.sse"),
    ]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.success().stdout(), "Hello there!\n");
    assert!(run.stderr().contains("retrying"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn billing_errors_fail_with_a_hint_and_no_retry() {
    let fake = Fake::start().await;
    fake.script([Reply::Error {
        status: 402,
        kind: "insufficient_balance",
        retry_after: None,
    }]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    let err = run.stderr();
    assert!(err.contains("Out of funds"), "{err}");
    assert!(err.contains("req_fake"), "{err}");
    assert!(err.contains("chat.crowbot.sh"), "{err}");
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_whose_body_stalls_is_retried_not_waited_on() {
    let fake = Fake::start().await;
    fake.script([Reply::Stall { status: 503 }, Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert!(run.success().stdout().contains("Hello there!"));
    assert!(run.stderr().contains("retrying"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mid_stream_error_keeps_the_partial_reply() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("error_midstream.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.stdout(), "Partial answ\n");
    assert!(run.stderr().contains("vendor refused"), "{}", run.stderr());
    // Output already streamed (and billed), so it is not retried.
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_length_stop_explains_the_cut() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("length.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert!(run.success().stderr().contains("output limit"));
}

#[tokio::test(flavor = "multi_thread")]
async fn effort_is_forwarded_and_validated() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    sandbox
        .run(&fake.url, &["--effort", "high", "-p", "hi"], WITH_KEY)
        .await
        .success();
    assert_eq!(fake.chat_bodies()[0]["reasoning_effort"], "high");

    // A model that does not reason gets no effort, and none is recorded as used.
    let args = [
        "--model",
        "fake-coder",
        "--effort",
        "high",
        "--json",
        "-p",
        "hi",
    ];
    let run = sandbox.run(&fake.url, &args, WITH_KEY).await;
    assert!(fake.chat_bodies()[1].get("reasoning_effort").is_none());
    let end = run
        .success()
        .stdout()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|e| e["type"] == "message_end")
        .unwrap();
    assert_eq!(end["message"]["effort"], Value::Null, "{end}");

    let bad = sandbox
        .run(&fake.url, &["--effort", "ludicrous", "-p", "hi"], WITH_KEY)
        .await;
    assert_eq!(bad.code(), Some(1));
    assert!(bad.stderr().contains("unknown effort"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_models_are_refused_before_spending() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let run = sandbox
        .run(&fake.url, &["--model", "nope", "-p", "hi"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("unknown model"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_key_it_says_how_to_log_in() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], None).await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("crowbot login"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flag_after_the_prompt_is_refused_not_sent() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(&fake.url, &["-p", "hi", "--json"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
    assert!(
        run.stderr().contains("`--json` goes before the words"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_command_on_the_command_line_names_where_it_works() {
    let fake = Fake::start().await;
    let run = Sandbox::default().run(&fake.url, &["mode"], WITH_KEY).await;
    assert_ne!(run.code(), Some(0));
    assert!(
        run.stderr().contains("`mode` only works as `/mode`"),
        "{}",
        run.stderr()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_that_starts_with_a_command_name_is_not_dropped() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(&fake.url, &["help", "me", "fix", "the", "build"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("usage: crowbot help"),
        "{}",
        run.stderr()
    );
    assert!(!run.stdout().contains("Commands:"), "{}", run.stdout());
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_with_nothing_to_send_says_so() {
    let fake = Fake::start().await;
    let run = Sandbox::default().run(&fake.url, &["-p"], WITH_KEY).await;
    assert_ne!(run.code(), Some(0));
    assert!(run.stderr().contains("nothing to send"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}
