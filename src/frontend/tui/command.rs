//! Session commands, run as futures beside input so a slow one (`/models --refresh`) never
//! freezes the screen.

use futures_util::future::BoxFuture;

use crate::agent::roles::RoleId;
use crate::api::account::{self, Me};
use crate::api::error::ApiError;
use crate::app::App;
use crate::commands::{self, Ctx, Missing, Outcome, Scope, Session};
use crate::frontend::tui::ui;
use crate::models::{self, Catalog};
use crate::text::template;

/// What finished in the background.
pub enum Work {
    Command(anyhow::Result<Outcome>),
    /// A fresh list, and the role the card it was fetched for opens on.
    Catalog(Catalog, Option<RoleId>),
    /// The account as the key of generation `changes` (`Api::key_changes`) sees it.
    Account {
        changes: u64,
        result: Result<Me, ApiError>,
    },
}

/// Starts `line` (what followed the `/`); `Err` is the notice for a command that does not run
/// here.
pub fn start<'a>(
    app: &'a App,
    line: &str,
    session: Session,
) -> Result<BoxFuture<'a, Work>, String> {
    let (name, args) = commands::split(line).ok_or_else(String::new)?;
    let command = commands::lookup(&name, Scope::Session).map_err(|missing| {
        let text = &ui::get().text;
        let typed = Scope::Session.invoke(&name);
        match missing {
            Missing::Unknown => template::fill(
                &text.unknown_command,
                &[
                    ("command", &typed),
                    ("help", &Scope::Session.invoke("help")),
                ],
            ),
            Missing::Elsewhere(spec) => commands::elsewhere(spec, Scope::Session),
        }
    })?;
    Ok(Box::pin(async move {
        let cx = Ctx {
            app,
            scope: Scope::Session,
            session: Some(session),
        };
        Work::Command(command.run(&cx, &args).await)
    }))
}

/// A fresh model list, fetched without holding up input.
pub fn refresh_catalog(app: &App, role: Option<RoleId>) -> BoxFuture<'_, Work> {
    Box::pin(async move { Work::Catalog(models::load(&app.api, &app.paths, true).await, role) })
}

/// Where the account stands now, tagged with the key it was asked with.
pub fn refresh_account(app: &App) -> BoxFuture<'_, Work> {
    let changes = app.api.key_changes();
    Box::pin(async move {
        Work::Account {
            changes,
            result: account::me(&app.api).await,
        }
    })
}
