use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Spec};
use crate::app::App;
use crate::io::term::{self, Event, KeyCode, KeyEventKind};

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/keytest.toml")));

pub struct KeyTest;

impl Command for KeyTest {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        _app: &'a App,
        _args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async {
            tokio::task::spawn_blocking(probe).await??;
            Ok(String::new())
        })
    }
}

/// Echoes every terminal event until Esc is pressed twice in a row.
fn probe() -> std::io::Result<()> {
    let raw = term::Raw::enter()?;
    // Raw mode turns off newline translation, hence the explicit carriage returns.
    term::out(&format!(
        "keyboard enhancement: {}\r\nPress keys (try Shift+Tab, Shift+Enter, a multi-line paste). Esc twice quits.\r\n",
        raw.enhanced()
    ));
    let mut escapes = 0;
    loop {
        let event = term::read_event()?;
        term::out(&format!("{event:?}\r\n"));
        if let Event::Key(key) = event
            && key.kind == KeyEventKind::Press
        {
            escapes = if key.code == KeyCode::Esc {
                escapes + 1
            } else {
                0
            };
            if escapes == 2 {
                return Ok(());
            }
        }
    }
}
