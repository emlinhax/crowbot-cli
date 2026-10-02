//! The interactive session through a real pseudo-terminal, read back through a terminal emulator.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::fake_crowbot::{ENV_KEY, Fake, Reply};
use crate::{CLEARED, Sandbox, launch_env};

const ROWS: u16 = 30;
const COLS: u16 = 100;
/// Longest wait for something to appear on screen or go away.
const WAIT: Duration = Duration::from_secs(20);
/// Longest wait for crowbot to exit once asked to.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// How often the screen is checked while waiting.
const POLL: Duration = Duration::from_millis(50);
/// Between typed keys: far above `tui.paste_gap_ms` (data/limits.toml), so typing never looks
/// like a paste. CEILING: keys are timestamped when crowbot reads them, so a stall longer than
/// two gaps can still bunch three into a paste (tui/paste.rs).
const KEY_GAP: Duration = Duration::from_millis(30);

struct Session {
    screen: vt100::Parser,
    /// Every byte crowbot wrote, for what the emulator does not show (OSC 52).
    written: Vec<u8>,
    output: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Held so the child keeps its terminal; dropping it would hang it up.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

/// A failed test must not leave crowbot running.
impl Drop for Session {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}

impl Session {
    fn start(sandbox: &Sandbox, api_url: &str) -> Self {
        Self::start_as(sandbox, api_url, Some(ENV_KEY), &[])
    }

    /// `key: None` starts logged out; `args` go on the command line.
    fn start_as(sandbox: &Sandbox, api_url: &str, key: Option<&str>, args: &[&str]) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_crowbot"));
        cmd.args(args);
        cmd.cwd(sandbox.project.path());
        for name in CLEARED {
            cmd.env_remove(name);
        }
        for (name, value) in launch_env(sandbox.home.path(), api_url, key) {
            cmd.env(name, value);
        }
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd).expect("crowbot starts");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let writer = pty.master.take_writer().unwrap();
        Self {
            screen: vt100::Parser::new(ROWS, COLS, 500),
            written: Vec::new(),
            output,
            writer,
            child,
            _master: pty.master,
        }
    }

    fn send(&mut self, bytes: &str) {
        self.writer.write_all(bytes.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    /// Types like a person: one key at a time, far slower than a paste.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(&c.to_string());
            std::thread::sleep(KEY_GAP);
        }
    }

    fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    fn bottom_row(&self) -> String {
        self.row(usize::from(ROWS) - 1)
    }

    /// Feeds output to the emulator, answering cursor-position queries as a real terminal
    /// would: Windows' ConPTY asks one before it lets any output through. True if any came.
    fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok(bytes) = self.output.try_recv() {
            any = true;
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                let (row, col) = self.screen.screen().cursor_position();
                self.send(&format!("\x1b[{};{}R", row + 1, col + 1));
            }
            self.screen.process(&bytes);
            self.written.extend(bytes);
        }
        any
    }

    /// Pumps until `done` answers, or fails after `limit` naming `what` and the screen.
    #[track_caller]
    fn poll<T>(
        &mut self,
        limit: Duration,
        what: &str,
        mut done: impl FnMut(&mut Self) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            self.pump();
            if let Some(found) = done(self) {
                return found;
            }
            std::thread::sleep(POLL);
        }
        panic!("{what}; screen:\n{}", self.contents());
    }

    #[track_caller]
    fn wait_for(&mut self, text: &str) {
        self.poll(WAIT, &format!("never saw {text:?}"), |s| {
            s.contents().contains(text).then_some(())
        });
    }

    #[track_caller]
    fn wait_gone(&mut self, text: &str) {
        self.poll(WAIT, &format!("{text:?} never went away"), |s| {
            (!s.contents().contains(text)).then_some(())
        });
    }

    /// Until one poll interval passes with no new output: the screen has settled.
    #[track_caller]
    fn wait_quiet(&mut self) {
        self.poll(WAIT, "output never settled", |s| {
            std::thread::sleep(POLL);
            (!s.pump()).then_some(())
        });
    }

    /// Row `i` of the screen, zero-based.
    fn row(&self, i: usize) -> String {
        self.screen
            .screen()
            .rows(0, COLS)
            .nth(i)
            .unwrap_or_default()
    }

    /// The screen row showing `text`, zero-based.
    #[track_caller]
    fn row_of(&self, text: &str) -> usize {
        self.screen
            .screen()
            .rows(0, COLS)
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no row shows {text:?}; screen:\n{}", self.contents()))
    }

    /// A left click as a terminal reports it (SGR mouse encoding, one-based).
    fn click(&mut self, row: usize) {
        self.press(row, 0);
    }

    fn right_click(&mut self, row: usize) {
        self.press(row, 2);
    }

    fn press(&mut self, row: usize, button: u8) {
        self.send(&format!(
            "\x1b[<{button};3;{0}M\x1b[<{button};3;{0}m",
            row + 1
        ));
    }

    #[track_caller]
    fn wait_exit(&mut self) -> bool {
        self.poll(EXIT_WAIT, "crowbot did not exit", |s| {
            s.child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.success())
        })
    }
}

/// What a session left behind: `body`'s answer, the screen before and after quitting (the
/// session's own screen is gone once it exits), and the sandbox, for the files it wrote.
struct Ended<T> {
    out: T,
    screen: String,
    exited: String,
    sandbox: Sandbox,
}

/// Starts a session (logged out when `key` is `None`), waits for its welcome, runs `body`, then
/// quits with Ctrl+D and asserts a clean exit.
async fn in_session<T: Send + 'static>(
    fake: &Fake,
    sandbox: Sandbox,
    key: Option<&'static str>,
    body: impl FnOnce(&mut Session) -> T + Send + 'static,
) -> Ended<T> {
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut s = Session::start_as(&sandbox, &url, key, &[]);
        s.wait_for("crowbot v");
        let out = body(&mut s);
        let screen = s.contents();
        s.send("\x04");
        assert!(
            s.wait_exit(),
            "crowbot exited with an error; screen:\n{}",
            s.contents()
        );
        Ended {
            out,
            screen,
            exited: s.contents(),
            sandbox,
        }
    })
    .await
    .unwrap()
}

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
        s.wait_for("AUTO");
    })
    .await;
    assert!(ended.exited.contains("Session saved"), "{}", ended.exited);
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

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

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_folder_is_asked_about_once_before_its_config_loosens_anything() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.copy_project("untrusted_auto");
    let url = fake.url.clone();
    let (screens, trusted) = tokio::task::spawn_blocking(move || {
        let mut screens = Vec::new();
        for answer in ["n\r", "y\r", ""] {
            let mut s = Session::start(&sandbox, &url);
            if answer.is_empty() {
                s.wait_for("AUTO");
            } else {
                s.wait_for("Trust this folder");
                s.send(answer);
                s.wait_for(if answer == "y\r" { "AUTO" } else { "MANUAL" });
            }
            s.send("\x04");
            assert!(s.wait_exit(), "{}", s.contents());
            screens.push(s.contents());
        }
        let trusted = std::fs::read_to_string(sandbox.home.path().join("trusted.json")).unwrap();
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
async fn a_command_typed_during_a_run_never_reaches_the_model() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("pty/sleep.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home.path()).unwrap();
    std::fs::write(sandbox.home.path().join("config.toml"), "mode = \"auto\"\n").unwrap();
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("AUTO");
        s.type_text("go");
        s.send("\r");
        s.wait_for("sleep 3");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
        s.wait_for("Hello there!");
        s.contents()
    })
    .await;
    // The command ran mid-turn, echoed with its number hidden.
    assert!(ended.out.contains("/login --key …3456"), "{}", ended.out);
    let sessions: String = ended
        .sandbox
        .sessions()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let bodies = serde_json::to_string(&fake.chat_bodies()).unwrap();
    assert_eq!(fake.hits("/v1/chat/completions"), 2);
    for secret in ["9012", "3456"] {
        assert!(
            !bodies.contains(secret),
            "{secret} reached the model: {bodies}"
        );
        assert!(!sessions.contains(secret), "{secret} was saved: {sessions}");
    }
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
async fn manual_mode_shows_the_edit_and_applies_it_once_approved() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("scenarios/auto_fix/1.sse"),
        Reply::sse("scenarios/auto_fix/2.sse"),
        Reply::sse("scenarios/auto_fix/4.sse"),
    ]);
    let sandbox = Sandbox::default();
    sandbox.copy_project("calc");
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("fix the sum");
        s.send("\r");
        s.wait_for("edit src/calc.txt");
        s.wait_for("+2 + 2 = 4");
        s.wait_for("1 Yes");
        s.send("1");
        s.wait_for("Fixed: 2 + 2 = 4.");
    })
    .await;
    let fixed = std::fs::read_to_string(ended.sandbox.project.path().join("src/calc.txt")).unwrap();
    assert_eq!(fixed, "2 + 2 = 4\n", "{}", ended.screen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slash_opens_the_command_popup_which_completes_and_runs() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("/mo");
        s.wait_for("╭ commands");
        s.wait_for("/models");
        // Tab completes the first match and leaves room for arguments; the popup closes.
        s.send("\t");
        s.wait_gone("╭ commands");
        // The bar never leaves the bottom row, whatever opened and closed above it.
        assert!(s.bottom_row().contains("crow-2"), "{}", s.contents());
        s.send("\r");
        s.wait_for("AUTO");
        // Down picks the second match and Enter runs it.
        s.type_text("/mo");
        s.wait_for("╭ commands");
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("╭ Models");
        s.wait_for("fake-coder");
        s.wait_gone("╭ commands");
        let picked = s.contents();
        // ← closes the picker, like Esc.
        s.send("\x1b[D");
        s.wait_gone("╭ Models");
        picked
    })
    .await;
    assert!(ended.out.contains("/models"), "{}", ended.out);
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_pairs_this_device_and_the_session_chats_at_once() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    fake.script([Reply::sse("hello.sse")]);
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to crowbot");
        s.send("1");
        s.wait_for("ABCD-1234");
        s.wait_for("Logged in with device key …9999");
        // The key is live in this session: no restart needed.
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert!(ended.screen.contains("$12.30"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_a_masked_account_number_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login");
        s.send("\r");
        s.wait_for("Log in to crowbot");
        s.send("2");
        s.type_text("0000 0000 0000 0000");
        s.send("\r");
        s.wait_for("invalid_api_key");
        s.send("2");
        s.type_text("1234 5678 9012 3456");
        s.wait_for("•••• •••• •••• 3456");
        assert!(!s.contents().contains("1234 5678"), "{}", s.contents());
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
    })
    .await;
    assert!(!ended.screen.contains("1234 5678"), "{}", ended.screen);
    assert!(ended.sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_the_same_arguments_as_the_command_line() {
    let fake = Fake::start().await;
    let ended = in_session(&fake, Sandbox::default(), None, |s| {
        s.wait_for("type /login");
        s.type_text("/login --status");
        s.send("\r");
        s.wait_for("Not logged in. Run /login");
        s.type_text("/login --key 1234 5678 9012 3456");
        s.send("\r");
        s.wait_for("Logged in with account number …3456");
    })
    .await;
    assert!(ended.sandbox.home().join("auth.json").exists());
}

// Elsewhere a system clipboard is always there, and a test must not overwrite the developer's;
// here DISPLAY is cleared, so the copy goes out as OSC 52, which the test can read.
#[cfg(target_os = "linux")]
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

// Linux only, as above.
#[cfg(target_os = "linux")]
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

#[tokio::test(flavor = "multi_thread")]
async fn slash_models_picks_the_model_for_the_rest_of_the_session() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("/models");
        s.send("\r");
        s.wait_for("╭ Models");
        // The list opens on the model in use; the next one down is picked with Enter.
        s.send("\x1b[B");
        s.send("\r");
        s.wait_for("Model: fake-coder for this session.");
        assert!(s.bottom_row().contains("fake-coder"), "{}", s.contents());
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
    })
    .await;
    assert_eq!(fake.chat_bodies()[0]["model"], "fake-coder");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_popup_floats_over_the_conversation_without_moving_it() {
    let fake = Fake::start().await;
    in_session(&fake, Sandbox::default(), Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        // More output than the screen holds, so the conversation fills every row above the bar.
        for _ in 0..3 {
            s.type_text("/help");
            s.send("\r");
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
        s.send("\x7f");
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
