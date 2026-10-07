//! Chatting, switching mode, thinking blocks and the context share.

use super::harness::{WAIT, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_session_welcomes_chats_switches_mode_and_quits() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        // The message bar and the rule under it, with the model, sit at the bottom from the start.
        assert!(s.bottom_row().contains("crow-2"), "{}", s.contents());
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.send("\x1b[Z");
        s.poll(WAIT, "Shift+Tab never reached PLAN", |s| {
            s.bottom_row().contains("PLAN").then_some(())
        });
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
        s.wait_for("MANUAL");
        s.type_text("//etc/hosts is odd");
        s.send("\r");
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
        s.wait_for("MANUAL");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.wait_for("▸ Thought for");
        assert!(!s.contents().contains("The user greets me."));
        s.send("\x14");
        s.wait_for("The user greets me.");
        s.send("\x14");
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
        s.wait_for("MANUAL");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Read it all.");
        // 100k tokens: a tenth of crow-2's million, three quarters of fake-coder's 131k.
        s.poll(WAIT, "ctx 10% on crow-2", |s| {
            s.bottom_row().contains("ctx 10%").then_some(())
        });
        s.type_text("/models");
        s.send("\r");
        s.wait_for("╭ Models");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Model: fake-coder for this session.");
        s.poll(WAIT, "ctx 76% on fake-coder", |s| {
            s.bottom_row().contains("ctx 76%").then_some(())
        });
    })
    .await;
}
