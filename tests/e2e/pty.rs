//! The interactive session through a real pseudo-terminal, read back through a terminal emulator.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, Fake, Reply};

const ROWS: u16 = 30;
const COLS: u16 = 100;

struct Session {
    screen: vt100::Parser,
    output: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Session {
    fn start(sandbox: &Sandbox, api_url: &str) -> Self {
        Self::start_as(sandbox, api_url, Some(ENV_KEY))
    }

    /// `key: None` starts logged out.
    fn start_as(sandbox: &Sandbox, api_url: &str, key: Option<&str>) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_crowbot"));
        cmd.cwd(sandbox.project.path());
        for (key, value) in [
            ("CROWBOT_HOME", sandbox.home.path().to_str().unwrap()),
            ("CROWBOT_API_URL", api_url),
            ("CROWBOT_CHAT_URL", api_url),
            ("CROWBOT_NO_BROWSER", "1"),
            ("TERM", "xterm-256color"),
        ] {
            cmd.env(key, value);
        }
        match key {
            Some(key) => cmd.env("CROWBOT_API_KEY", key),
            None => cmd.env_remove("CROWBOT_API_KEY"),
        }
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
        // The master must outlive the session or the child sees a hangup.
        std::mem::forget(pty.master);
        Self {
            screen: vt100::Parser::new(ROWS, COLS, 500),
            output,
            writer,
            child,
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
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    fn bottom_row(&self) -> String {
        self.screen
            .screen()
            .rows(0, COLS)
            .last()
            .unwrap_or_default()
    }

    /// Feeds output to the emulator, answering cursor-position queries as a real terminal
    /// would: Windows' ConPTY asks one before it lets any output through.
    fn pump(&mut self) {
        while let Ok(bytes) = self.output.try_recv() {
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                let (row, col) = self.screen.screen().cursor_position();
                self.send(&format!("\x1b[{};{}R", row + 1, col + 1));
            }
            self.screen.process(&bytes);
        }
    }

    #[track_caller]
    fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            self.pump();
            if self.contents().contains(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("never saw {text:?}; screen:\n{}", self.contents());
    }

    #[track_caller]
    fn wait_gone(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            self.pump();
            if !self.contents().contains(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{text:?} never went away; screen:\n{}", self.contents());
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
        self.send(&format!("\x1b[<0;3;{0}M\x1b[<0;3;{0}m", row + 1));
    }

    fn wait_exit(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.pump();
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("crowbot did not exit; screen:\n{}", self.contents());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_welcomes_chats_switches_mode_and_quits() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    let hits_after = tokio::task::spawn_blocking(move || {
        let mut s = Session::start(&sandbox, &url);
        s.wait_for("crowbot v");
        s.wait_for("MANUAL");
        // The message bar and the rule under it, with the model, sit at the bottom from the start.
        assert!(s.bottom_row().contains("crow-2"), "{}", s.contents());
        s.type_text("hi");
        s.send("\r");
        s.wait_for("Hello there!");
        s.send("\x1b[Z");
        s.wait_for("AUTO");
        s.send("\x04");
        let clean = s.wait_exit();
        (clean, s.contents())
    })
    .await
    .unwrap();
    let (clean, screen) = hits_after;
    assert!(clean, "crowbot exited with an error; screen:\n{screen}");
    assert!(screen.contains("Session saved"), "{screen}");
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
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
    let url = fake.url.clone();
    let (screen, sandbox) = tokio::task::spawn_blocking(move || {
        let mut s = Session::start(&sandbox, &url);
        s.wait_for("MANUAL");
        s.type_text("fix the sum");
        s.send("\r");
        s.wait_for("edit src/calc.txt");
        s.wait_for("+2 + 2 = 4");
        s.wait_for("1 Yes");
        s.send("1");
        s.wait_for("Fixed: 2 + 2 = 4.");
        s.send("\x04");
        s.wait_exit();
        (s.contents(), sandbox)
    })
    .await
    .unwrap();
    let fixed = std::fs::read_to_string(sandbox.project.path().join("src/calc.txt")).unwrap();
    assert_eq!(fixed, "2 + 2 = 4\n", "{screen}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slash_opens_the_command_popup_which_completes_and_runs() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    let screen = tokio::task::spawn_blocking(move || {
        let mut s = Session::start(&sandbox, &url);
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
        s.wait_for("fake-coder");
        s.wait_gone("╭ commands");
        // The session screen goes when crowbot exits, so read it first.
        let screen = s.contents();
        s.send("\x04");
        s.wait_exit();
        screen
    })
    .await
    .unwrap();
    assert!(screen.contains("/models"), "{screen}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_pairs_this_device_and_the_session_chats_at_once() {
    let fake = Fake::start().await;
    fake.pair_after(1);
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    let (screen, sandbox) = tokio::task::spawn_blocking(move || {
        let mut s = Session::start_as(&sandbox, &url, None);
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
        let screen = s.contents();
        s.send("\x04");
        s.wait_exit();
        (screen, sandbox)
    })
    .await
    .unwrap();
    assert!(screen.contains("$12.30"), "{screen}");
    assert!(sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_login_takes_a_masked_account_number_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    let (screen, sandbox) = tokio::task::spawn_blocking(move || {
        let mut s = Session::start_as(&sandbox, &url, None);
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
        let screen = s.contents();
        s.send("\x04");
        s.wait_exit();
        (screen, sandbox)
    })
    .await
    .unwrap();
    assert!(!screen.contains("1234 5678"), "{screen}");
    assert!(sandbox.home().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn thinking_collapses_opens_on_click_and_ctrl_t_toggles_it_all() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut s = Session::start(&sandbox, &url);
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
        s.send("\x04");
        s.wait_exit();
    })
    .await
    .unwrap();
}
