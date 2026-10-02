//! Session commands, run as futures beside input so a slow one (`/models --refresh`) never
//! freezes the screen.

use futures_util::future::BoxFuture;

use crate::api::models::{self, Catalog};
use crate::app::App;
use crate::commands::{self, Ctx, Missing, Outcome, Scope, Session};
use crate::text::template;
use crate::tui::ui;

/// What finished in the background.
pub enum Work {
    Command(anyhow::Result<Outcome>),
    Catalog(Catalog),
}

pub struct Started<'a> {
    /// The line as the transcript shows it, secrets hidden.
    pub echo: String,
    pub run: BoxFuture<'a, Work>,
}

/// Starts `line` (what followed the `/`); `Err` is the notice for a command that does not run
/// here.
pub fn start<'a>(app: &'a App, line: &str, session: Session) -> Result<Started<'a>, String> {
    let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    let (name, args) = words.split_first().ok_or_else(String::new)?;
    let command = commands::lookup(name, Scope::Session).map_err(|missing| {
        let text = &ui::get().text;
        let typed = Scope::Session.invoke(name);
        match missing {
            Missing::Unknown => template::fill(
                &text.unknown_command,
                &[
                    ("command", &typed),
                    ("help", &Scope::Session.invoke("help")),
                ],
            ),
            Missing::Elsewhere(spec) => template::fill(
                &text.elsewhere,
                &[("command", &typed), ("places", &commands::places(spec))],
            ),
        }
    })?;
    let args = args.to_vec();
    Ok(Started {
        echo: Scope::Session.invoke(&commands::redact_line(line)),
        run: Box::pin(async move {
            let cx = Ctx {
                app,
                scope: Scope::Session,
                session: Some(session),
            };
            Work::Command(command.run(&cx, &args).await)
        }),
    })
}

/// A fresh model list, fetched without holding up input.
pub fn refresh_catalog(app: &App) -> BoxFuture<'_, Work> {
    Box::pin(async move { Work::Catalog(models::load(app, true).await) })
}
