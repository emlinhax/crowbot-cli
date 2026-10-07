//! Copying to the clipboard, which here goes out as OSC 52.

use super::harness::{Session, WAIT, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

impl Session {
    pub fn right_click(&mut self, row: usize) {
        self.press(row, 2);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_a_reply_and_says_so_at_the_top_right() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        let row = s.row_of("Hello there!");
        s.right_click(row);
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "Hello there!" in base64, as OSC 52 carries it.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;SGVsbG8gdGhlcmUh\x07");
        assert!(osc, "no OSC 52 with the reply");
        s.poll(WAIT, "the copied note stayed", |s| {
            (!s.row(0).contains("✓ Copied")).then_some(())
        });
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_copy_copies_the_last_reply() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("/copy");
        s.send("\r");
        s.wait_for("Nothing to copy yet");
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.type_text("/copy");
        s.send("\r");
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "Hello there!" in base64.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;SGVsbG8gdGhlcmUh\x07");
        assert!(osc, "no OSC 52 with the reply");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_the_pairing_code_from_its_card() {
    let fake = Fake::start().await;
    fake.pair_after(100);
    in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to crowbot");
        s.send("1");
        s.wait_for("right-click copies it");
        s.right_click(2);
        s.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        // "ABCD-1234" in base64.
        let osc = String::from_utf8_lossy(&s.written).contains("\x1b]52;c;QUJDRC0xMjM0\x07");
        assert!(osc, "no OSC 52 with the code");
        s.send("\x1b");
        s.wait_gone("Log in to crowbot");
    })
    .await;
}
