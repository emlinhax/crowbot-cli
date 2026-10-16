//! Chatting, switching mode, thinking blocks and the context share.

use super::harness::{CTRL_C, CTRL_T, DOWN, ENTER, SHIFT_TAB, in_session, in_session_with};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_session_welcomes_chats_switches_mode_and_quits() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        // The message bar and the rule under it, with the model, sit at the bottom from the start.
        assert!(s.bottom_row().contains("crow-2"), "{}", s.contents());
        s.submit("hi");
        s.wait_for("Hello there!");
        s.send(SHIFT_TAB);
        s.wait_bar("PLAN");
    })
    .await;
    assert!(ended.exited.contains("Session saved"), "{}", ended.exited);
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_doubled_slash_sends_a_message_that_starts_with_a_slash() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("//etc/hosts is odd");
        s.wait_for("Hello there!");
    })
    .await;
    let bodies = fake.chat_bodies();
    assert_eq!(bodies[0]["messages"][1]["content"], "/etc/hosts is odd");
}

#[tokio::test(flavor = "multi_thread")]
async fn thinking_collapses_opens_on_click_and_ctrl_t_toggles_it_all() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Hello there!");
        s.wait_for("▸ Thought for");
        assert!(!s.contents().contains("The user greets me."));
        s.send(CTRL_T);
        s.wait_for("The user greets me.");
        s.send(CTRL_T);
        s.wait_gone("The user greets me.");
        let row = s.row_of("▸ Thought for");
        s.click(row);
        s.wait_for("The user greets me.");
        s.click(row);
        s.wait_gone("The user greets me.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_context_share_follows_a_switch_of_model_at_once() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("pty/long_context.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Read it all.");
        // 100k tokens: a tenth of crow-2's million, three quarters of fake-coder's 131k.
        s.wait_bar("ctx 10%");
        s.submit("/models");
        s.wait_for("╭ Models");
        s.send(DOWN);
        s.send(ENTER);
        s.wait_for("Model: fake-coder for this session.");
        s.wait_bar("ctx 76%");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_reopens_the_last_conversation_with_its_mode() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::text("The second answer.")]);
    let first = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.send(SHIFT_TAB);
        s.wait_bar("PLAN");
        s.submit("the first question");
        s.wait_for("Hello there!");
    })
    .await;
    let ended = in_session_with(&fake, first.sandbox, Some(ENV_KEY), &["--continue"], |s| {
        s.wait_for("the first question");
        s.wait_for("Going on with");
        s.wait_bar("PLAN");
        s.submit("the second question");
        s.wait_for("The second answer.");
    })
    .await;
    let second = fake.chat_bodies()[1]["messages"].to_string();
    assert!(second.contains("the first question"), "{second}");
    assert_eq!(ended.sandbox.sessions().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_takes_back_the_last_prompt_for_good() {
    let fake = Fake::start().await;
    fake.script([
        Reply::text("The first answer."),
        Reply::text("An answer to take back."),
        Reply::text("The answer after."),
    ]);
    let first = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/undo");
        s.wait_for("Nothing to take back");
        s.submit("first");
        s.wait_for("The first answer.");
        s.submit("second");
        s.wait_for("An answer to take back.");
        s.submit("/undo");
        s.wait_for("Took back the last prompt");
        assert!(
            !s.contents().contains("An answer to take back."),
            "{}",
            s.contents()
        );
        // The prompt is back in the editor; quit with it unsent.
        s.wait_for("second");
        s.send(CTRL_C);
    })
    .await;
    in_session_with(&fake, first.sandbox, Some(ENV_KEY), &["--continue"], |s| {
        s.wait_for("The first answer.");
        assert!(!s.contents().contains("take back"), "{}", s.contents());
        s.submit("third");
        s.wait_for("The answer after.");
    })
    .await;
    let third = fake.chat_bodies()[2]["messages"].to_string();
    assert!(
        third.contains("first") && third.contains("third"),
        "{third}"
    );
    assert!(!third.contains("second"), "{third}");
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_puts_back_the_files_the_prompt_changed() {
    let fake = Fake::start().await;
    fake.script([
        Reply::Calls(vec![
            crate::fake_crowbot::Call {
                name: "bash".into(),
                arguments: r#"{"command": "printf two > a.txt"}"#.into(),
            },
            crate::fake_crowbot::Call {
                name: "write".into(),
                arguments: r#"{"path": "b.txt", "content": "new"}"#.into(),
            },
        ]),
        Reply::text("Changed them."),
    ]);
    let sandbox = Sandbox::default();
    sandbox.write_home("config.toml", "mode = \"auto\"\n");
    sandbox.write_project("a.txt", "one");
    let git = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(sandbox.project())
        .status();
    if !git.is_ok_and(|s| s.success()) {
        return;
    }
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.submit("change things");
        s.wait_for("Changed them.");
        s.submit("/undo");
        s.wait_for("2 files it changed are back as they were.");
        // The prompt is back in the editor; clear it to quit.
        s.send(CTRL_C);
    })
    .await;
    let project = ended.sandbox.project();
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "one"
    );
    assert!(!project.join("b.txt").exists());
    let session = std::fs::read_to_string(&ended.sandbox.sessions()[0]).unwrap();
    assert_eq!(
        session.matches(r#""type":"snapshot""#).count(),
        2,
        "{session}"
    );
}
