//! Headless mode: one prompt in, the agent runs to completion, the reply (or a JSON event
//! stream) out. Permission prompts cannot be answered here, so they are declined.

use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::anyhow;

use crate::agent::event::{AgentEvent, DeltaKind, Outcome};
use crate::agent::message::{Assistant, Finish, Part};
use crate::agent::prompt::{Prompt, Reply};
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::models;
use crate::app::App;
use crate::io::term;
use crate::mode;
use crate::permission::gate;
use crate::session::store::Store;
use crate::session::transcript::Transcript;
use crate::text::shorten;
use crate::tools::{self, Registry};

/// How much of an argument or error a one-line note shows.
const NOTE_CHARS: usize = 100;

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
    let mode = mode::get(&app.settings.mode).ok_or_else(|| anyhow!("mode was validated"))?;
    let system = system_prompt::build(&app.paths, model);
    let registry = Registry::builtin(app);
    let mut transcript = Transcript::new(Some(Store::create(&app.paths)?));
    let shared = Shared::new(mode);
    let plan_file = tools::plan_file(&app.paths, &transcript.id());

    let out = Printer {
        format,
        shared: &shared,
        at_line_start: AtomicBool::new(true),
        last: Mutex::new(None),
    };
    let emit = |event: AgentEvent| out.event(event);
    let cx = RunCtx {
        app,
        model,
        effort: app.settings.effort.as_deref(),
        system: &system,
        tools: &registry,
        plan_file,
        emit: &emit,
    };

    let interrupt = async {
        term::interrupted().await;
        shared.interrupt();
        std::future::pending::<()>().await;
    };
    let prompt = vec![Part::Text { text: prompt }];
    let outcome = tokio::select! {
        outcome = run::run(&cx, &mut transcript, &shared, prompt) => outcome,
        () = interrupt => unreachable!("the interrupt future never finishes"),
    };
    Ok(out.finish(outcome))
}

struct Printer<'a> {
    format: Format,
    shared: &'a Shared,
    /// Whether stdout ends at a line start, so stderr notes do not land mid-sentence.
    at_line_start: AtomicBool,
    last: Mutex<Option<Assistant>>,
}

impl Printer<'_> {
    fn event(&self, event: AgentEvent) {
        // Nobody can answer a prompt here; decline it so the model hears why and carries on.
        if let AgentEvent::Prompt { id, .. } = &event {
            self.shared.answer(*id, Reply::Unavailable);
        }
        if let AgentEvent::MessageEnd { message } = &event {
            *self.last.lock().unwrap() = Some(message.clone());
        }
        match self.format {
            Format::Json => {
                if let Ok(line) = serde_json::to_string(&event) {
                    term::out(&format!("{line}\n"));
                }
            }
            Format::Text => self.text(&event),
        }
    }

    fn text(&self, event: &AgentEvent) {
        match event {
            AgentEvent::Delta {
                kind: DeltaKind::Text,
                text,
                ..
            } => {
                term::out(text);
                self.at_line_start
                    .store(text.ends_with('\n'), Ordering::Relaxed);
            }
            AgentEvent::ToolStart {
                name, arguments, ..
            } => self.note(&format!("› {name} {}", summary(arguments))),
            AgentEvent::ToolEnd { result } if result.is_error => {
                let first = result.content.lines().next().unwrap_or_default();
                self.note(&format!("  ✗ {}", shorten::line(first, NOTE_CHARS)));
            }
            AgentEvent::Prompt {
                prompt: Prompt::Permission { tool, asks, .. },
                ..
            } => self.note(&format!(
                "  ✗ {tool} needs permission ({}); rerun with --mode auto to allow it",
                shorten::line(&gate::describe(asks), NOTE_CHARS)
            )),
            AgentEvent::Retry {
                delay_ms, error, ..
            } => self.note(&format!(
                "{}; retrying in {:.1}s",
                error.entry().title,
                *delay_ms as f64 / 1000.0
            )),
            AgentEvent::Notice { text } => self.note(text),
            AgentEvent::Unsaved { path, error } => {
                self.note(&format!(
                    "crowbot: no longer saving this session to {path}: {error}"
                ));
            }
            _ => {}
        }
    }

    fn note(&self, line: &str) {
        if !self.at_line_start.swap(true, Ordering::Relaxed) {
            term::out("\n");
        }
        term::err(&format!("{line}\n"));
    }

    fn finish(&self, outcome: Outcome) -> ExitCode {
        if self.format == Format::Text && !self.at_line_start.load(Ordering::Relaxed) {
            term::out("\n");
        }
        let last = self.last.lock().unwrap().take();
        match outcome {
            Outcome::Aborted => {
                term::err("crowbot: cancelled\n");
                ExitCode::from(130)
            }
            Outcome::Failed => {
                if let Some(error) = last.and_then(|m| m.error) {
                    let entry = error.entry();
                    term::err(&format!("crowbot: {}: {error}\n", entry.title));
                    if let Some(hint) = &entry.hint {
                        term::err(&format!("  {hint}\n"));
                    }
                }
                ExitCode::FAILURE
            }
            Outcome::Rejected | Outcome::TurnLimit => ExitCode::FAILURE,
            Outcome::Done => {
                if last.is_some_and(|m| m.finish == Finish::Length) {
                    term::err(
                        "crowbot: the reply hit its output limit (a low balance lowers that limit)\n",
                    );
                }
                ExitCode::SUCCESS
            }
        }
    }
}

/// The argument a person recognises a call by: its path, command, pattern, URL or query.
fn summary(arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let found = ["path", "command", "pattern", "url", "query"]
        .iter()
        .find_map(|key| args[key].as_str())
        .unwrap_or_default();
    shorten::line(found.lines().next().unwrap_or_default(), NOTE_CHARS)
}
