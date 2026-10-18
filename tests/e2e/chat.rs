use serde_json::Value;

use crate::fake_crowbot::{Fake, Reply};
use crate::{Sandbox, WITH_KEY, tests_path};

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
    assert_eq!(body["model"], "glm-5.3-flash-crow");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    // The first user message carries the opening orientation reminder, then the prompt.
    let first_user = body["messages"][1]["content"].as_str().unwrap();
    assert!(first_user.ends_with("hi there"), "{first_user}");
    assert!(first_user.contains("<workspace>"), "{first_user}");

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
    // 600 uncached * $0.20 + 400 cached * $0.10 + 100 out * $0.60, in micro-dollars
    assert_eq!(reply["usage"]["cost_micros"], 220);
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_goes_on_with_the_latest_session_in_its_own_file() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let none = sandbox
        .run(&fake.url, &["-p", "--continue", "hi"], WITH_KEY)
        .await;
    assert_eq!(none.code(), Some(1));
    none.stderr_has("no earlier session");

    sandbox
        .run(&fake.url, &["-p", "first"], WITH_KEY)
        .await
        .success();
    sandbox
        .run(&fake.url, &["-p", "--continue", "second"], WITH_KEY)
        .await
        .success();
    let second = fake.chat_bodies()[1]["messages"].to_string();
    assert!(
        second.contains("first") && second.contains("Hello there!") && second.contains("second"),
        "{second}"
    );
    let sessions = sandbox.sessions();
    assert_eq!(sessions.len(), 1);
    let text = std::fs::read_to_string(&sessions[0]).unwrap();
    // One header, then two prompts and their replies, each prompt with what it was sent under.
    assert_eq!(text.lines().count(), 5);
    assert_eq!(
        text.matches(r#""state":{"model":"glm-5.3-flash-crow""#)
            .count(),
        2
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn json_mode_emits_one_event_per_line() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["--json", "hi"], WITH_KEY).await;
    let stdout = run.success().stdout();
    run.events();
    // The stream is a contract for scripts: its golden changes only on purpose.
    let golden = tests_path("golden/json/hello.jsonl");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &stdout).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(stdout, want, "--json changed; if on purpose, `make golden`");
}

#[tokio::test(flavor = "multi_thread")]
async fn json_mode_nests_a_subagents_events_in_its_task_call() {
    let fake = Fake::start().await;
    fake.script([
        Reply::call(
            "task",
            r#"{"description": "look", "prompt": "find it", "subagent_type": "explore"}"#,
        ),
        Reply::text("found it"),
        Reply::text("It is found."),
    ]);
    let sandbox = Sandbox::default();
    let run = sandbox.run(&fake.url, &["--json", "go"], WITH_KEY).await;
    // The subagent's file has a fresh name each run; everything else is the contract.
    let stdout: String = run
        .success()
        .stdout()
        .lines()
        .map(|line| {
            let mut event: Value = serde_json::from_str(line).unwrap();
            if let Some(session) = event.pointer_mut("/result/details/session") {
                *session = Value::from("{session}");
            }
            format!("{event}\n")
        })
        .collect();
    let golden = tests_path("golden/json/task.jsonl");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &stdout).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(stdout, want, "--json changed; if on purpose, `make golden`");
    let nested = stdout.lines().any(|line| {
        let event: Value = serde_json::from_str(line).unwrap();
        event["type"] == "child" && event["event"]["type"] == "message_end"
    });
    assert!(nested, "{stdout}");
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
        .events()
        .into_iter()
        .find(|e| e["type"] == "message_end")
        .unwrap();
    assert_eq!(end["message"]["effort"], Value::Null, "{end}");

    let bad = sandbox
        .run(&fake.url, &["--effort", "ludicrous", "-p", "hi"], WITH_KEY)
        .await;
    assert_eq!(bad.code(), Some(1));
    bad.stderr_has("unknown effort");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_development_build_says_it_has_no_release_to_update_from() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let update = sandbox.run(&fake.url, &["update"], None).await;
    assert_eq!(update.code(), Some(1));
    update.stderr_has("development build");
    let version = sandbox.run(&fake.url, &["--version"], None).await;
    let out = version.success().stdout();
    assert_eq!(out.trim(), format!("crowbot {}", env!("CARGO_PKG_VERSION")));
}
