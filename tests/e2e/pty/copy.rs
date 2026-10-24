//! Copying to the clipboard. Linux only: DISPLAY is cleared, so a copy goes out as OSC 52, which
//! the test reads. Elsewhere it would hit the developer's clipboard.

use super::harness::{CTRL_C, ESC, Session, WAIT, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

impl Session {
    pub fn right_click(&mut self, row: usize) {
        self.press(row, 2);
    }

    /// The left button pressed on `from`, dragged to `to` and let go there, as (row, column)
    /// zero-based (SGR mouse encoding, one-based).
    pub fn drag(&mut self, from: (usize, usize), to: (usize, usize)) {
        let at = |(row, column): (usize, usize)| format!("{};{}", column + 1, row + 1);
        let (from, to) = (at(from), at(to));
        self.send(&format!("[<0;{from}M[<32;{to}M[<0;{to}m"));
    }

    /// The zero-based column where `text` starts on screen row `row`.
    #[track_caller]
    pub fn column_of(&self, row: usize, text: &str) -> usize {
        let shown = self.row(row);
        let at = shown
            .find(text)
            .unwrap_or_else(|| panic!("row {row} is {shown:?}"));
        shown[..at].chars().count()
    }

    pub fn highlighted(&self, row: usize, column: usize) -> bool {
        let screen = self.screen.screen();
        screen
            .cell(row as u16, column as u16)
            .is_some_and(|cell| cell.inverse())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_a_reply_and_says_so_at_the_top_right() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Hello there!");
        let row = s.row_of("Hello there!");
        s.right_click(row);
        s.wait_copied("Hello there!");
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
        s.submit("/copy");
        s.wait_for("Nothing to copy yet");
        s.submit("hi");
        s.wait_for("Hello there!");
        s.submit("/copy");
        s.wait_copied("Hello there!");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn right_click_copies_the_pairing_code_from_its_card() {
    let fake = Fake::start().await;
    fake.pair_after(100);
    in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.submit("/login");
        s.wait_for("Log in to crowbot");
        s.send("1");
        s.wait_for("right-click copies it");
        s.right_click(2);
        s.wait_copied("ABCD-1234");
        s.send(ESC);
        s.wait_gone("Log in to crowbot");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drag_highlights_and_copies_what_it_covers_and_ctrl_c_copies_it_again() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.submit("hi");
        s.wait_for("Hello there!");
        s.type_text("draft");
        let row = s.row_of("Hello there!");
        let column = s.column_of(row, "Hello");
        s.drag((row, column), (row, column + 4));
        s.wait_copied("Hello");
        s.poll(WAIT, "the drag was never highlighted", |s| {
            s.highlighted(row, column + 4).then_some(())
        });
        assert!(!s.highlighted(row, column + 5));
        // On a highlight, Ctrl+C copies instead of clearing the editor, and the highlight goes.
        s.wait_gone("✓ Copied");
        s.written.clear();
        s.send(CTRL_C);
        s.wait_copied("Hello");
        s.poll(WAIT, "the highlight stayed", |s| {
            (!s.highlighted(row, column)).then_some(())
        });
        assert!(s.contents().contains("› draft"), "{}", s.contents());
        // With the highlight gone, Ctrl+C clears the editor again.
        s.send(CTRL_C);
        s.wait_gone("› draft");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_right_click_away_from_any_message_and_ctrl_v_paste_the_clipboard() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        // DISPLAY is cleared, so there is no clipboard to read, and both say so.
        let bar = s.row_of("Ask crowbot anything");
        s.right_click(bar);
        s.poll(WAIT, "no note for an empty clipboard", |s| {
            s.row(0).contains("Nothing to paste").then_some(())
        });
        s.wait_gone("Nothing to paste");
        s.send("");
        s.poll(WAIT, "no note for Ctrl+V", |s| {
            s.row(0).contains("Nothing to paste").then_some(())
        });
    })
    .await;
}
