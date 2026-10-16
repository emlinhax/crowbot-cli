//! A crowbot session in a pseudo-terminal: starting it, typing, reading its screen, quitting.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::fake_crowbot::Fake;
use crate::{Sandbox, cleared, launch_env};

pub const ROWS: u16 = 30;
pub const COLS: u16 = 100;
/// Longest wait for something to appear on screen or go away.
pub const WAIT: Duration = Duration::from_secs(20);
/// Longest wait for crowbot to exit once asked to.
pub const EXIT_WAIT: Duration = Duration::from_secs(10);
/// How often the screen is checked while waiting.
pub const POLL: Duration = Duration::from_millis(50);
/// Between typed keys: far above `tui.paste_gap_ms` (data/limits.toml), so typing never looks
/// like a paste. CEILING: keys are timestamped when crowbot reads them, so a stall longer than
/// two gaps can still bunch three into a paste (frontend/tui/paste.rs).
pub const KEY_GAP: Duration = Duration::from_millis(30);

/// Keys as a terminal sends them.
pub const ENTER: &str = "\r";
pub const TAB: &str = "\t";
pub const ESC: &str = "\x1b";
pub const DOWN: &str = "\x1b[B";
pub const LEFT: &str = "\x1b[D";
pub const SHIFT_TAB: &str = "\x1b[Z";
pub const CTRL_C: &str = "\x03";
pub const CTRL_D: &str = "\x04";
pub const CTRL_T: &str = "\x14";
pub const BACKSPACE: &str = "\x7f";

pub struct Session {
    pub screen: vt100::Parser,
    /// Every byte crowbot wrote, for what the emulator does not show (OSC 52).
    pub written: Vec<u8>,
    pub output: mpsc::Receiver<Vec<u8>>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Held so the child keeps its terminal; dropping it would hang it up.
    pub _master: Box<dyn portable_pty::MasterPty + Send>,
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
    /// `key: None` starts logged out; `args` go on the command line.
    pub fn start_as(sandbox: &Sandbox, api_url: &str, key: Option<&str>, args: &[&str]) -> Self {
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
        cmd.cwd(sandbox.project());
        for name in cleared() {
            cmd.env_remove(name);
        }
        for (name, value) in launch_env(sandbox.home(), Some(api_url), key) {
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

    pub fn send(&mut self, bytes: &str) {
        self.writer.write_all(bytes.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    /// Types like a person: one key at a time, far slower than a paste.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(&c.to_string());
            std::thread::sleep(KEY_GAP);
        }
    }

    /// Types `text` and presses Enter.
    pub fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.send(ENTER);
    }

    pub fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    pub fn bottom_row(&self) -> String {
        self.row(usize::from(ROWS) - 1)
    }

    /// Feeds output to the emulator, answering cursor-position queries as a real terminal
    /// would: Windows' ConPTY asks one before it lets any output through. True if any came.
    pub fn pump(&mut self) -> bool {
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
    pub fn poll<T>(
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
    pub fn wait_for(&mut self, text: &str) {
        self.poll(WAIT, &format!("never saw {text:?}"), |s| {
            s.contents().contains(text).then_some(())
        });
    }

    #[track_caller]
    pub fn wait_gone(&mut self, text: &str) {
        self.poll(WAIT, &format!("{text:?} never went away"), |s| {
            (!s.contents().contains(text)).then_some(())
        });
    }

    /// Until the bar under the editor shows `text`.
    #[track_caller]
    pub fn wait_bar(&mut self, text: &str) {
        self.poll(WAIT, &format!("the bar never showed {text:?}"), |s| {
            s.bottom_row().contains(text).then_some(())
        });
    }

    /// Until the bar under the editor no longer shows `text`.
    #[track_caller]
    pub fn wait_bar_gone(&mut self, text: &str) {
        self.poll(WAIT, &format!("the bar kept showing {text:?}"), |s| {
            (!s.bottom_row().contains(text)).then_some(())
        });
    }

    /// Until the copied note shows, then that `text` went to the clipboard (as OSC 52 carries
    /// it, base64). The copy tests that read it run on Linux only.
    #[cfg(target_os = "linux")]
    #[track_caller]
    pub fn wait_copied(&mut self, text: &str) {
        use base64::Engine;
        self.poll(WAIT, "no copied note", |s| {
            s.row(0).contains("✓ Copied").then_some(())
        });
        let carried = base64::engine::general_purpose::STANDARD.encode(text);
        let osc = format!("\x1b]52;c;{carried}\x07");
        let sent = String::from_utf8_lossy(&self.written).contains(&osc);
        assert!(sent, "no OSC 52 carrying {text:?}");
    }

    /// Until one poll interval passes with no new output: the screen has settled.
    #[track_caller]
    pub fn wait_quiet(&mut self) {
        self.poll(WAIT, "output never settled", |s| {
            std::thread::sleep(POLL);
            (!s.pump()).then_some(())
        });
    }

    /// Row `i` of the screen, zero-based.
    pub fn row(&self, i: usize) -> String {
        self.screen
            .screen()
            .rows(0, COLS)
            .nth(i)
            .unwrap_or_default()
    }

    /// The screen row showing `text`, zero-based.
    #[track_caller]
    pub fn row_of(&self, text: &str) -> usize {
        self.screen
            .screen()
            .rows(0, COLS)
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no row shows {text:?}; screen:\n{}", self.contents()))
    }

    /// A left click as a terminal reports it (SGR mouse encoding, one-based).
    pub fn click(&mut self, row: usize) {
        self.press(row, 0);
    }

    pub fn press(&mut self, row: usize, button: u8) {
        self.send(&format!(
            "\x1b[<{button};3;{0}M\x1b[<{button};3;{0}m",
            row + 1
        ));
    }

    #[track_caller]
    pub fn wait_exit(&mut self) -> bool {
        self.poll(EXIT_WAIT, "crowbot did not exit", |s| {
            s.child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.success())
        })
    }
}

/// After a session: `body`'s result, the screen before and after quitting, and the sandbox.
pub struct Ended<T> {
    pub out: T,
    pub screen: String,
    pub exited: String,
    pub sandbox: Sandbox,
}

/// Starts a session (logged out when `key` is `None`), waits for its welcome and its bar, runs
/// `body`, then quits with Ctrl+D and asserts a clean exit.
pub async fn in_session<T: Send + 'static>(
    fake: &Fake,
    sandbox: Sandbox,
    key: Option<&'static str>,
    body: impl FnOnce(&mut Session) -> T + Send + 'static,
) -> Ended<T> {
    in_session_with(fake, sandbox, key, &[], body).await
}

/// `in_session`, started with `args` on the command line.
pub async fn in_session_with<T: Send + 'static>(
    fake: &Fake,
    sandbox: Sandbox,
    key: Option<&'static str>,
    args: &'static [&'static str],
    body: impl FnOnce(&mut Session) -> T + Send + 'static,
) -> Ended<T> {
    let url = fake.url.clone();
    tokio::task::spawn_blocking(move || {
        let mut s = Session::start_as(&sandbox, &url, key, args);
        s.wait_for("crowbot v");
        // The bar names the mode and the model once the session is ready for keys.
        s.wait_bar(" · ");
        let out = body(&mut s);
        let screen = s.contents();
        s.send(CTRL_D);
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
