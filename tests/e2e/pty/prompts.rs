//! Cards that wait on the user: trust, permission, the model's questions.

use super::harness::{CTRL_D, ENTER, Session, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_folder_is_asked_about_once_before_its_config_loosens_anything() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.copy_project("calc");
    sandbox.write_project(".crowbot/config.toml", "mode = \"auto\"\n");
    let url = fake.url.clone();
    let (screens, trusted) = tokio::task::spawn_blocking(move || {
        let mut screens = Vec::new();
        for answer in ["n\r", "y\r", ""] {
            let mut s = Session::start_as(&sandbox, &url, Some(ENV_KEY), &[]);
            if answer.is_empty() {
                s.wait_for("AUTO");
            } else {
                s.wait_for("Trust this folder");
                s.send(answer);
                s.wait_for(if answer == "y\r" { "AUTO" } else { "MANUAL" });
            }
            s.send(CTRL_D);
            assert!(s.wait_exit(), "{}", s.contents());
            screens.push(s.contents());
        }
        let trusted = std::fs::read_to_string(sandbox.home().join("trusted.json")).unwrap();
        (screens, trusted)
    })
    .await
    .unwrap();
    assert!(screens[0].contains("mode = \"auto\""), "{}", screens[0]);
    assert!(screens[0].contains("Not trusted"), "{}", screens[0]);
    assert!(!screens[2].contains("Trust this folder"), "{}", screens[2]);
    let trusted: serde_json::Value = serde_json::from_str(&trusted).unwrap();
    assert_eq!(
        trusted["projects"].as_array().unwrap().len(),
        1,
        "{trusted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_mode_shows_the_edit_and_applies_it_once_approved() {
    let fake = Fake::start().await;
    fake.script([
        Reply::call("read", r#"{"path": "src/calc.txt"}"#),
        Reply::call(
            "edit",
            r#"{"path": "src/calc.txt", "edits": [{"old_text": "2 + 2 = 5", "new_text": "2 + 2 = 4"}]}"#,
        ),
        Reply::text("Fixed: 2 + 2 = 4."),
    ]);
    let sandbox = Sandbox::default();
    sandbox.copy_project("calc");
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.submit("fix the sum");
        s.wait_for("edit src/calc.txt");
        s.wait_for("+2 + 2 = 4");
        s.wait_for("1 Yes");
        s.send("1");
        s.wait_for("Fixed: 2 + 2 = 4.");
    })
    .await;
    let fixed = std::fs::read_to_string(ended.sandbox.project().join("src/calc.txt")).unwrap();
    assert_eq!(fixed, "2 + 2 = 4\n", "{}", ended.screen);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_models_questions_are_answered_on_one_card() {
    let fake = Fake::start().await;
    let questions = r#"{"questions": [
        {"question": "Which database should the app use?", "header": "Database", "options": [
            {"label": "SQLite", "description": "one file, nothing to run"},
            {"label": "Postgres", "description": "what production runs"}]},
        {"question": "Which checks run before a push?", "header": "Checks",
            "options": ["lint", "test", "e2e"], "multiple": true}]}"#;
    fake.script([Reply::call("question", questions), Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("set it up");
        s.wait_for("Which database should the app use?");
        s.wait_for("what production runs");
        s.send("2");
        s.wait_for("Which checks run before a push?");
        s.send("1");
        s.wait_for("[x] lint");
        s.send("3");
        s.wait_for("[x] e2e");
        s.send(ENTER);
        s.wait_for("Your answers");
        s.wait_for("lint, e2e");
        s.send(ENTER);
        s.wait_for("Hello there!");
    })
    .await;
    let answered = fake.chat_bodies()[1]["messages"].to_string();
    assert!(answered.contains("Postgres"), "{answered}");
    assert!(answered.contains("lint, e2e"), "{answered}");
}

fn task_fetching(fake: &Fake, after: &[Reply]) {
    let mut replies = vec![
        Reply::call(
            "task",
            r#"{"description": "fetch the page", "prompt": "fetch it", "subagent_type": "explore"}"#,
        ),
        Reply::call("webfetch", r#"{"url": "{base}/web/ok"}"#),
    ];
    replies.extend(after.iter().map(|r| match r {
        Reply::Text(t) => Reply::text(t),
        _ => unreachable!("text replies only"),
    }));
    fake.script(replies);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_permission_card_is_answered_in_the_session() {
    let fake = Fake::start().await;
    task_fetching(
        &fake,
        &[Reply::text("CHILD: fetched"), Reply::text("Parent: done.")],
    );
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("fetch it for me");
        s.wait_for("╭ webfetch");
        s.send("1");
        s.wait_for("Parent: done.");
        assert!(!s.contents().contains("CHILD: fetched"), "{}", s.contents());
        s.wait_for("explore: 1 call");
    })
    .await;
    assert_eq!(fake.hits("/web/ok"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_requests_count_in_the_sessions_cost() {
    let fake = Fake::start().await;
    fake.script([
        Reply::text("One."),
        Reply::call(
            "task",
            r#"{"description": "look", "prompt": "look", "subagent_type": "explore"}"#,
        ),
        Reply::text("child"),
        Reply::text("Two."),
    ]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("first");
        s.wait_for("One.");
        let one = cost(&s.bottom_row());
        s.submit("second");
        s.wait_for("Two.");
        // Three more requests, the subagent's among them.
        s.poll(super::harness::WAIT, "cost never added up", |s| {
            (cost(&s.bottom_row()) == one * 4.0).then_some(())
        });
    })
    .await;
}

/// The session's cost as the bar shows it.
fn cost(bar: &str) -> f64 {
    let at = bar.find(" est").expect("the bar shows a cost");
    let start = bar[..at].rfind('$').unwrap() + 1;
    bar[start..at].parse().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_down_a_subagents_call_stops_the_run() {
    for decline in ["2", "\x1b"] {
        let fake = Fake::start().await;
        task_fetching(&fake, &[Reply::text("never")]);
        in_session(&fake, Sandbox::default(), Some(ENV_KEY), move |s| {
            s.submit("fetch it for me");
            s.wait_for("╭ webfetch");
            s.send(decline);
            s.wait_for("The user turned down one of the subagent's calls");
        })
        .await;
        assert_eq!(fake.hits("/v1/chat/completions"), 2, "{decline:?}");
        assert_eq!(fake.hits("/web/ok"), 0);
    }
}

/// The editor is a script that reads a line from the terminal and adds it to the plan, so what is
/// typed reaches it only if the session let go of the terminal; then the session takes it back.
/// Linux only: elsewhere the sandbox's home is reached through a link, and the plan's path with it.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_edited_in_the_users_editor_is_what_the_model_implements() {
    use std::os::unix::fs::PermissionsExt;

    let fake = Fake::start().await;
    fake.script([
        Reply::call(
            "write",
            r#"{"path": "{plan_file}", "content": "1. fix the sum\n"}"#,
        ),
        Reply::call("plan_exit", "{}"),
        Reply::text("Implementing."),
    ]);
    let mut sandbox = Sandbox::default();
    fake.set_home(sandbox.home());
    sandbox.write_home("config.toml", "mode = \"plan\"\n");
    let editor = sandbox.user().join("editor.sh");
    let script =
        "#!/bin/sh\nprintf 'EDITING> '\nIFS= read -r line\nprintf '%s\\n' \"$line\" >> \"$1\"\n";
    std::fs::write(&editor, script).unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox.set_env("VISUAL", &editor.display().to_string());
    in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_bar("PLAN");
        s.submit("fix the sum");
        s.wait_for("Plan ready");
        s.wait_for("1. fix the sum");
        // After the three choices in data/tools/plan_exit.toml.
        s.send("4");
        s.wait_for("EDITING>");
        s.send("2. and test it\r");
        s.wait_for("The plan as you left it");
        s.wait_for("2. and test it");
        s.send("1");
        s.wait_for("Implementing.");
    })
    .await;
    let approved = fake.chat_bodies()[2]["messages"].to_string();
    assert!(approved.contains("edited the plan"), "{approved}");
    assert!(approved.contains("2. and test it"), "{approved}");
    assert!(approved.contains("MANUAL"), "{approved}");
}
