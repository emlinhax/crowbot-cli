//! Headless run: one prompt in, the reply (or one JSON event per line) out. Nobody can answer a
//! prompt here.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::bail;

use crate::agent::setup::{Continue, Setup};
use crate::agent::system_prompt;
use crate::agent::{RunCtx, run};
use crate::api::error::{self, Entry};
use crate::app::App;
use crate::config::limits;
use crate::conversation::message::{Assistant, Finish, Part, ToolResult};
use crate::frontend::{tool_card, words};
use crate::io::term;
use crate::live::event::{AgentEvent, DeltaKind, Ended, Outcome};
use crate::live::prompt::{Prompt, Reply};
use crate::live::state::Shared;
use crate::mode;
use crate::permission::gate;
use crate::text::shorten;
use crate::text::template::fill;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

/// Runs `prompt` (the words given, then any piped stdin) without a session, and exits; going on
/// with the session in `resume` when given.
pub async fn run(
    app: &App,
    mut prompt: String,
    format: Format,
    resume: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    if let Some(piped) = term::piped_stdin()? {
        prompt = if prompt.is_empty() {
            piped
        } else {
            format!("{prompt}\n\n{piped}")
        };
    }
    if prompt.trim().is_empty() {
        bail!("nothing to send; pass a prompt or pipe one in");
    }
    let from = resume.map(|path| Continue { path, mode: false });
    let Setup {
        model,
        effort,
        tools,
        shared,
        mut transcript,
        gone,
        ..
    } = Setup::load(app, from).await?;
    if let Some(gone) = gone {
        term::warn(&words::model_gone(&gone, &model.id));
    }
    let system = system_prompt::build(&app.paths, &model.id);

    let out = Printer {
        format,
        shared: &shared,
        at_line_start: AtomicBool::new(true),
        last: Mutex::new(None),
        declined: Mutex::new(Vec::new()),
    };
    let emit = |event: AgentEvent| out.event(event);
    let cx = RunCtx {
        app,
        model: &model,
        effort: effort.as_deref(),
        system: &system,
        tools: &tools,
        emit: &emit,
    };

    let interrupt = async {
        term::interrupted().await;
        shared.interrupt();
        std::future::pending::<()>().await;
    };
    let prompt = vec![Part::Text { text: prompt }];
    let ended = tokio::select! {
        ended = run::run(&cx, &mut transcript, &shared, prompt) => ended,
        () = interrupt => unreachable!("the interrupt future never finishes"),
    };
    Ok(out.finish(ended))
}

struct Printer<'a> {
    format: Format,
    shared: &'a Shared,
    /// Whether stdout ends at a line start, so stderr notes do not land mid-sentence.
    at_line_start: AtomicBool,
    last: Mutex<Option<Assistant>>,
    /// Calls already reported at their declined prompt; their start and end say nothing new.
    declined: Mutex<Vec<String>>,
}

impl Printer<'_> {
    fn event(&self, event: AgentEvent) {
        // Declined at once, so the model hears why and carries on.
        if let AgentEvent::Prompt { id, .. } = &event {
            self.shared.prompts.answer(*id, Reply::Unavailable);
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
            AgentEvent::ToolStart { call_id, .. }
            | AgentEvent::ToolEnd {
                result: ToolResult { call_id, .. },
            } if self.declined.lock().unwrap().contains(call_id) => {}
            AgentEvent::ToolStart {
                name, arguments, ..
            } => {
                let mut line = fill(&words::get().print.call, &[("tool", name)]);
                if let Some(title) = tool_card::title(name, arguments) {
                    line.push(' ');
                    line.push_str(&shorten::line(&title, note_chars()));
                }
                self.note(&line);
            }
            AgentEvent::ToolEnd { result } if result.is_error => {
                let first = result.content.lines().next().unwrap_or_default();
                let error = shorten::line(first, note_chars());
                self.note(&fill(&words::get().print.failed, &[("error", &error)]));
            }
            AgentEvent::Prompt {
                prompt:
                    Prompt::Permission {
                        tool,
                        asks,
                        confirm,
                        ..
                    },
                call_id,
                ..
            } => {
                self.declined.lock().unwrap().push(call_id.clone());
                let what = shorten::line(&gate::describe(asks), note_chars());
                let print = &words::get().print;
                let line = if *confirm {
                    &print.unconfirmed
                } else {
                    &print.needs_permission
                };
                self.note(&fill(
                    line,
                    &[
                        ("tool", tool),
                        ("what", &what),
                        ("mode", &mode::unattended().id),
                    ],
                ));
            }
            AgentEvent::Retry {
                delay_ms, error, ..
            } => self.note(&format!(
                "{}; {} {:.1}s",
                Entry::of(error).title,
                words::get().retrying,
                *delay_ms as f64 / 1000.0
            )),
            AgentEvent::Unsaved { path, error } => {
                self.warn(&fill(
                    &words::get().unsaved,
                    &[("path", path), ("error", error)],
                ));
            }
            _ => {}
        }
    }

    fn note(&self, line: &str) {
        self.break_line();
        term::err(&format!("{line}\n"));
    }

    fn warn(&self, line: &str) {
        self.break_line();
        term::warn(line);
    }

    /// Ends a reply's half-written line, so what goes to stderr starts on its own.
    fn break_line(&self) {
        if !self.at_line_start.swap(true, Ordering::Relaxed) {
            term::out("\n");
        }
    }

    fn finish(&self, Ended { outcome, turns }: Ended) -> ExitCode {
        if self.format == Format::Text && !self.at_line_start.load(Ordering::Relaxed) {
            term::out("\n");
        }
        let last = self.last.lock().unwrap().take();
        match outcome {
            Outcome::Aborted => {
                term::warn(&words::get().interrupted);
                ExitCode::from(130)
            }
            Outcome::Failed => {
                if let Some(error) = last.and_then(|m| m.error) {
                    let entry = Entry::of(&error);
                    term::warn(&error::headline(&error));
                    if let Some(hint) = entry.hint() {
                        let hint = fill(&words::get().print.hint, &[("hint", &hint)]);
                        term::err(&format!("{hint}\n"));
                    }
                }
                ExitCode::FAILURE
            }
            Outcome::TurnLimit => {
                let count = turns.to_string();
                term::warn(&fill(&words::get().turn_limit, &[("count", &count)]));
                ExitCode::FAILURE
            }
            Outcome::Rejected => ExitCode::FAILURE,
            Outcome::Done => {
                if last.is_some_and(|m| m.finish == Finish::Length) {
                    term::warn(&words::get().cut_off);
                }
                ExitCode::SUCCESS
            }
        }
    }
}

fn note_chars() -> usize {
    limits::get().print.note_chars.value
}
