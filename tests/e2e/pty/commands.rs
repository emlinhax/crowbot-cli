//! Session commands and the command popup.

use serde_json::json;

use super::harness::{BACKSPACE, DOWN, ENTER, ESC, LEFT, TAB, in_session};
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
        assert!(
            s.bottom_row().contains("deepseek-v4.1-flash"),
            "{}",
            s.contents()
        );
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
async fn slash_status_shows_the_balance_plan_model_and_mode() {
    let fake = Fake::start().await;
    fake.set_me(
        json!({"balance_microdollars": 12_300_000, "can_spend": true,
        "pass": {"label": "Pro", "allowance_bp": 6375, "refills": "3 hours"}}),
    );
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/status");
        s.wait_for("Balance $12.30");
        let screen = s.contents();
        assert!(screen.contains("Plan Pro: 63%"), "{screen}");
        assert!(screen.contains("Model: deepseek-v4.1-flash"), "{screen}");
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
        s.wait_bar("deepseek-v4.1-flash high");
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
    let agents = sandbox.project().join("AGENTS.md");
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
async fn a_preset_picked_in_slash_models_picks_each_request_and_is_remembered() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    sandbox.write_home(
        "presets/thrifty.toml",
        "description = \"cheap\"\n[[rule]]\nwhen = { mode = [\"plan\"] }\nmodel = \"crow-2\"\n\
         [[rule]]\nmodel = \"fake-coder\"\n",
    );
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.submit("/models");
        s.wait_for("╭ Models");
        s.wait_for("presets");
        // Past the four models, the heading is stepped over onto the preset.
        for _ in 0..4 {
            s.send(DOWN);
        }
        // Keys that close on each other read as a paste; Enter waits for the selection to land.
        s.wait_for("›    thrifty");
        s.send(ENTER);
        s.wait_for("Model: thrifty for this session.");
        s.submit("hi");
        s.wait_for("Hello there!");
        s.wait_bar("thrifty → fake-coder");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "fake-coder");
    let prefs = std::fs::read_to_string(ended.sandbox.home().join("prefs.json")).unwrap();
    assert!(prefs.contains("\"thrifty\""), "{prefs}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_presets_new_writes_one_that_slash_models_then_lists() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/presets new cheap");
        s.wait_for("cheap.toml: edit it");
        s.submit("/models");
        s.wait_for("╭ Models");
        s.wait_for("cheap");
        s.wait_for("+ new preset");
        s.send(ESC);
        s.wait_gone("╭ Models");
    })
    .await;
    let text = std::fs::read_to_string(ended.sandbox.home().join("presets/cheap.toml")).unwrap();
    // Its one rule is on the session's model, so it can be picked as written.
    assert!(text.contains("model = \"deepseek-v4.1-flash\""), "{text}");
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

#[tokio::test(flavor = "multi_thread")]
async fn slash_resume_goes_on_with_an_earlier_session_and_keeps_this_one() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("hello.sse"),
        Reply::text("A second session."),
        Reply::text("Back in the first."),
    ]);
    let first = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("/resume");
        s.wait_for("No earlier session");
        s.submit("the first question");
        s.wait_for("Hello there!");
    })
    .await;
    let ended = in_session(&fake, first.sandbox, Some(ENV_KEY), |s| {
        s.submit("another question");
        s.wait_for("A second session.");
        s.submit("/resume");
        s.wait_for("╭ Sessions");
        s.wait_for("the first question");
        s.send(ENTER);
        s.wait_gone("╭ Sessions");
        s.wait_for("Going on with");
        s.wait_for("The last conversation is saved in");
        assert!(
            !s.contents().contains("another question"),
            "{}",
            s.contents()
        );
        s.submit("and now");
        s.wait_for("Back in the first.");
    })
    .await;
    let third = fake.chat_bodies()[2]["messages"].to_string();
    assert!(third.contains("the first question"), "{third}");
    assert!(!third.contains("another question"), "{third}");
    assert_eq!(ended.sandbox.sessions().len(), 2);
    assert_eq!(
        ended.exited.matches("Session saved").count(),
        2,
        "{}",
        ended.exited
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_compact_sends_a_summary_and_the_recent_turns_word_for_word() {
    let fake = Fake::start().await;
    fake.script([
        Reply::call("read", r#"{"path": "big1.txt"}"#),
        Reply::call("read", r#"{"path": "big2.txt"}"#),
        Reply::call("read", r#"{"path": "big3.txt"}"#),
        Reply::text("Read all three."),
        Reply::text("SUMMARY: the user had three big files read."),
        Reply::text("Carrying on."),
    ]);
    let sandbox = Sandbox::default();
    sandbox.write_home("config.toml", "mode = \"auto\"\n");
    for (name, word) in [
        ("big1.txt", "FIRST"),
        ("big2.txt", "SECOND"),
        ("big3.txt", "THIRD"),
    ] {
        let line = format!("{} {word}\n", "x".repeat(90));
        sandbox.write_project(name, &line.repeat(1200));
    }
    in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.submit("/compact");
        s.wait_for("Nothing to summarise yet");
        s.submit("read the files");
        s.wait_done("Read all three.");
        s.submit("/compact");
        s.wait_for("Summarised 3 earlier messages");
        s.submit("go on");
        s.wait_for("Carrying on.");
    })
    .await;
    let bodies = fake.chat_bodies();
    let asked = bodies[4]["messages"].to_string();
    assert!(
        asked.contains("FIRST") && asked.contains("call no tools"),
        "{asked}"
    );
    let next = bodies[5]["messages"].to_string();
    assert!(
        next.contains("SUMMARY: the user had three big files read."),
        "{next}"
    );
    assert!(next.contains("SECOND") && next.contains("THIRD"), "{next}");
    assert!(
        !next.contains("FIRST"),
        "the summarised read was sent again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_status_names_the_mcp_servers_and_how_they_stand() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let server = env!("CARGO_BIN_EXE_crowbot-fake-mcp").replace('\\', "/");
    sandbox.write_home(
        "config.toml",
        &format!("[mcp.fake]\ncommand = '{server}'\n[mcp.gone]\ncommand = 'no-such-mcp'\n"),
    );
    in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("MCP server gone is not running");
        s.submit("/status");
        s.wait_for("MCP: fake, 7 tools; gone not running (no-such-mcp was not found)");
    })
    .await;
}
