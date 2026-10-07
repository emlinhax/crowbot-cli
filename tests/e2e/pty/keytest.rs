//! `crowbot keytest` in a real terminal.

use super::harness::Session;
use crate::Sandbox;
use crate::fake_crowbot::Fake;

#[tokio::test(flavor = "multi_thread")]
async fn keytest_reads_the_terminal_and_leaves_it_as_it_found_it() {
    let fake = Fake::start().await;
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let sandbox = Sandbox::default();
        let mut s = Session::start_as(&sandbox, &url, None, &["keytest", "shift+tab"]);
        s.wait_for("Press Shift+Tab");
        assert!(s.screen.screen().bracketed_paste() && s.screen.screen().hide_cursor());
        s.send("\x1b[Z");
        s.wait_for("Shift+Tab: BackTab");
        assert!(s.wait_exit(), "{}", s.contents());
        let screen = s.screen.screen();
        assert!(!screen.bracketed_paste() && !screen.hide_cursor());
    })
    .await
    .unwrap();
}
