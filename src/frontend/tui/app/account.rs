//! What the session knows of the account, and when it asks again. Every refresh lands here,
//! whatever started it.

use std::time::Instant;

use futures_util::future::BoxFuture;

use super::Tui;
use crate::api::account::Me;
use crate::api::error::ApiError;
use crate::auth;
use crate::commands::account;
use crate::config::limits;
use crate::frontend::tui::command::{self, Work};
use crate::frontend::tui::ui;
use crate::io::clock;

#[derive(Default)]
pub(super) struct AccountView {
    /// As of the last refresh; `None` until one lands, and again once the key changes.
    pub me: Option<Me>,
    /// The key generation (`Api::key_changes`) `me` belongs to.
    changes: u64,
    /// A refresh is owed; it starts once none is in flight.
    due: bool,
    busy: bool,
    /// When the last refresh started, for the pace mid-run.
    at: Option<Instant>,
}

impl AccountView {
    /// Starts owing a refresh, so the first frames are followed by the account.
    pub fn new() -> Self {
        Self {
            due: true,
            ..Self::default()
        }
    }
}

impl<'a> Tui<'a> {
    /// The refresh to start now, if one is owed and none is in flight. Called once per pass of
    /// the loop, which is also where a changed key is noticed.
    pub(super) fn account_work(&mut self) -> Option<BoxFuture<'a, Work>> {
        let changes = self.app.api.key_changes();
        let view = &mut self.account;
        if changes != view.changes {
            view.changes = changes;
            view.me = None;
            view.due = true;
            self.feed.set_welcome(self.welcome());
        }
        let view = &mut self.account;
        if !self.app.api.has_key() {
            view.due = false;
            return None;
        }
        if !view.due || view.busy {
            return None;
        }
        view.due = false;
        view.busy = true;
        view.at = Some(clock::instant());
        Some(command::refresh_account(self.app))
    }

    pub(super) fn account_landed(&mut self, changes: u64, result: Result<Me, ApiError>) {
        let view = &mut self.account;
        view.busy = false;
        // Asked with a key that has since been replaced: ask again with this one.
        if changes != view.changes {
            view.due = true;
            return;
        }
        // A failed refresh keeps the last standing rather than blanking the bar.
        if let Ok(me) = result {
            view.me = Some(me);
            self.feed.set_welcome(self.welcome());
        }
    }

    /// What the welcome warns of, if anything stands in the way of chatting.
    pub(super) fn account_warning(&self) -> Option<String> {
        if !self.app.api.has_key() {
            // A key file that is there but unreadable says so, and how to clear it.
            return Some(match auth::stored(&self.app.paths) {
                Err(e) => format!("{e}"),
                Ok(_) => ui::get().text.not_logged_in.clone(),
            });
        }
        let me = self.account.me.as_ref()?;
        (!me.can_spend).then(account::cannot_spend)
    }

    /// A reply has ended mid-run: ask again if the last answer is old enough.
    pub(super) fn account_reply_ended(&mut self) {
        // CEILING: the balance is as of the last refresh; a top-up on the web shows at the next.
        // Upgrade: read it from the usage frame, once /v1 sends it as the chat door does.
        let pace = limits::get().tui.account_refresh_secs.secs();
        let view = &mut self.account;
        if view
            .at
            .is_none_or(|at| clock::instant().duration_since(at) >= pace)
        {
            view.due = true;
        }
    }

    pub(super) fn account_run_ended(&mut self) {
        self.account.due = true;
    }
}
