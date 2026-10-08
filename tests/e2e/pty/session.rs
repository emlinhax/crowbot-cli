//! Chatting, switching mode, thinking blocks and the context share.

use super::harness::{CTRL_T, DOWN, ENTER, SHIFT_TAB, in_session};
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
