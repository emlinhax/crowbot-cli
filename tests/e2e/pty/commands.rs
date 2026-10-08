//! Session commands and the command popup.

use super::harness::{BACKSPACE, DOWN, ENTER, LEFT, TAB, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_slash_opens_the_command_popup_which_completes_and_runs() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.type_text("/mo");
        s.wait_for("╭ commands");
        s.wait_for("/models");
        // Tab completes the first match and leaves room for arguments; the popup closes.
        s.send(TAB);
        s.wait_gone("╭ commands");
        // The bar never leaves the bottom row, whatever opened and closed above it.
        assert!(s.bottom_row().contains("crow-2"), "{}", s.contents());
        s.send(ENTER);
        s.wait_for("AUTO");
        // Down picks the second match and Enter runs it.
        s.type_text("/mo");
        s.wait_for("╭ commands");
        s.send(DOWN);
        s.send(ENTER);
        s.wait_for("╭ Models");
        s.wait_for("fake-coder");
        s.wait_gone("╭ commands");
        let picked = s.contents();
        // ← closes the picker, like Esc.
        s.send(LEFT);
        s.wait_gone("╭ Models");
        picked
    })
    .await;
    assert!(ended.out.contains("/models"), "{}", ended.out);
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_clear_starts_a_fresh_conversation_in_a_new_file() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("first question");
        s.wait_for("Hello there!");
        s.submit("/clear");
        s.wait_for("The last one is saved in");
        assert!(!s.contents().contains("first question"), "{}", s.contents());
        s.submit("second question");
        s.wait_for("Hello there!");
    })
    .await;
    let second = fake.chat_bodies()[1]["messages"].to_string();
    assert!(second.contains("second question"), "{second}");
    assert!(!second.contains("first question"), "{second}");
    assert_eq!(ended.sandbox.sessions().len(), 2);
    assert_eq!(
        ended.exited.matches("Session saved").count(),
        2,
        "{}",
        ended.exited
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_status_shows_the_balance_model_and_mode() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/status");
        s.wait_for("Balance $12.30");
        let screen = s.contents();
        assert!(screen.contains("Model: crow-2"), "{screen}");
        assert!(screen.contains("Mode: MANUAL"), "{screen}");
        assert!(screen.contains("nothing sent yet"), "{screen}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_effort_sets_what_the_next_requests_carry() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/effort ludicrous");
        s.wait_for("unknown effort");
        s.submit("/effort high");
        s.wait_for("Effort: high for this session.");
        s.wait_bar("crow-2 high");
        s.submit("hi");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["reasoning_effort"], "high");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_init_asks_for_an_agents_md_that_the_next_conversation_reads() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let agents = sandbox.project.path().join("AGENTS.md");
    in_session(&fake, sandbox, Some(ENV_KEY), move |s| {
        s.submit("/init");
        s.wait_for("Hello there!");
        // What the model would have written, then a conversation that starts after it.
        std::fs::write(&agents, "Run make test before a push.").unwrap();
        s.submit("/new");
        s.wait_for("A new conversation");
        s.submit("hi");
        s.wait_for("Hello there!");
    })
    .await;
    let bodies = fake.chat_bodies();
    let asked = bodies[0]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .to_string();
    assert!(asked.contains("Write an AGENTS.md"), "{asked}");
    let system = bodies[1]["messages"][0]["content"].to_string();
    assert!(system.contains("Run make test before a push."), "{system}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_models_picks_the_model_for_the_rest_of_the_session() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/models");
        s.wait_for("╭ Models");
        // The list opens on the model in use; the next one down is picked with Enter.
        s.send(DOWN);
        s.send(ENTER);
        s.wait_for("Model: fake-coder for this session.");
        assert!(s.bottom_row().contains("fake-coder"), "{}", s.contents());
        s.submit("hi");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "fake-coder");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_popup_floats_over_the_conversation_without_moving_it() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        // More output than the screen holds, so the conversation fills every row above the bar.
        for _ in 0..3 {
            s.submit("/help");
            s.wait_gone("╭ commands");
        }
        s.wait_quiet();
        let top: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        s.send("/");
        s.wait_for("╭ commands");
        let with_popup: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        assert_eq!(
            with_popup,
            top,
            "the conversation moved; screen:\n{}",
            s.contents()
        );
        s.send(BACKSPACE);
        s.wait_gone("╭ commands");
        let after: Vec<String> = (0..8).map(|i| s.row(i)).collect();
        assert_eq!(
            after,
            top,
            "the conversation moved; screen:\n{}",
            s.contents()
        );
    })
    .await;
}
