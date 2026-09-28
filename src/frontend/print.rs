//! Headless mode: one prompt in, the reply (or a JSON event stream) out.

use std::process::ExitCode;

use anyhow::anyhow;
use tokio_util::sync::CancellationToken;

use crate::agent::event::AgentEvent;
use crate::agent::message::{Finish, Message};
use crate::agent::system_prompt;
use crate::api::assemble::Delta;
use crate::api::chat::{self, Event, Turn};
use crate::api::models;
use crate::app::App;
use crate::io::term;
use crate::session::store::Store;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

pub async fn run(app: &App, prompt: String, format: Format) -> anyhow::Result<ExitCode> {
    let catalog = models::load(app, false).await;
    // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
    let model = catalog.get(&app.settings.model).ok_or_else(|| {
        anyhow!(
            "unknown model `{}`; see `crowbot models`",
            app.settings.model
        )
    })?;
    let system = system_prompt::build(&app.paths, model);
    let user = Message::user_text(prompt);
    let mut store = Store::create(&app.paths)?;
    store.append(&user)?;

    let cancel = CancellationToken::new();
    let on_interrupt = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            term::interrupted().await;
            cancel.cancel();
        })
    };

    let turn = Turn {
        model,
        effort: app.settings.effort.as_deref(),
        system: &system,
        messages: std::slice::from_ref(&user),
        tools: &[],
    };
    let mut ends_with_newline = true;
    let mut on = |event: Event<'_>| match format {
        Format::Json => emit(&match event {
            Event::Delta(delta) => AgentEvent::from(delta),
            Event::Retry {
                attempt,
                delay,
                error,
            } => AgentEvent::Retry {
                attempt,
                delay_ms: delay.as_millis() as u64,
                error: error.clone(),
            },
        }),
        Format::Text => match event {
            Event::Delta(Delta::Text(text)) => {
                term::out(text);
                ends_with_newline = text.ends_with('\n');
            }
            Event::Retry { delay, error, .. } => term::err(&format!(
                "crowbot: {}; retrying in {:.1}s\n",
                error.entry().title,
                delay.as_secs_f32()
            )),
            Event::Delta(_) => {}
        },
    };
    let reply = chat::stream(&app.api, &turn, &cancel, &mut on).await;
    on_interrupt.abort();
    store.append(&Message::Assistant(reply.clone()))?;

    if format == Format::Text && !ends_with_newline {
        term::out("\n");
    }
    let code = match (reply.finish, &reply.error) {
        (Finish::Aborted, _) => {
            term::err("crowbot: cancelled\n");
            ExitCode::from(130)
        }
        (_, Some(error)) => {
            let entry = error.entry();
            term::err(&format!("crowbot: {}: {error}\n", entry.title));
            if let Some(hint) = &entry.hint {
                term::err(&format!("  {hint}\n"));
            }
            ExitCode::FAILURE
        }
        (Finish::Length, None) => {
            term::err(
                "crowbot: the reply hit its output limit (a low balance lowers that limit)\n",
            );
            ExitCode::SUCCESS
        }
        _ => ExitCode::SUCCESS,
    };
    if format == Format::Json {
        emit(&AgentEvent::MessageEnd { message: reply });
    }
    Ok(code)
}

fn emit(event: &AgentEvent) {
    if let Ok(line) = serde_json::to_string(event) {
        term::out(&format!("{line}\n"));
    }
}
