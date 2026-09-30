use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand};

use crate::app::App;
use crate::commands::{self, Ctx, Scope};
use crate::frontend::print::{self, Format};
use crate::io::term;
use crate::settings::{Overrides, Untrusted};
use crate::{trust, tui};

#[derive(Parser)]
#[command(
    name = "crowbot",
    version,
    about = "Coding agent for crowbot.sh",
    allow_external_subcommands = true
)]
struct Cli {
    /// Model id for this run (see `crowbot models`).
    #[arg(long, global = true)]
    model: Option<String>,

    /// Reasoning effort for this run: low, medium, high or max.
    #[arg(long, global = true)]
    effort: Option<String>,

    /// Permission mode: manual (asks), auto (allows everything) or plan (read-only).
    #[arg(long, global = true)]
    mode: Option<String>,

    /// Send the prompt, print the reply, exit.
    #[arg(short, long)]
    print: bool,

    /// Like --print, but stream every event as a JSON line.
    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    command: Option<Sub>,
}

#[derive(Subcommand)]
enum Sub {
    /// A command from the registry (`crowbot help`), or else the words of a prompt.
    #[command(external_subcommand)]
    Words(Vec<String>),
}

pub async fn run() -> ExitCode {
    match dispatch(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            term::err(&format!("crowbot: {e:#}\n"));
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<ExitCode> {
    let flags = Overrides {
        model: cli.model,
        effort: cli.effort,
        mode: cli.mode,
    };
    let mut app = App::load(&flags)?;
    let words = match cli.command {
        Some(Sub::Words(words)) => words,
        None => Vec::new(),
    };
    let headless = cli.print || cli.json;
    let command = words
        .first()
        .and_then(|w| commands::find(w))
        .filter(|_| !headless);
    let session =
        command.is_none() && !headless && term::stdin_is_terminal() && term::stdout_is_terminal();
    if let Some(untrusted) = app.settings.untrusted.take() {
        if !session {
            term::err(&format!(
                "crowbot: {}\n",
                trust::ignored(&untrusted.file, &untrusted.keys)
            ));
        } else if confirm_trust(&untrusted)? {
            trust::trust(&app.paths)?;
            app = App::load(&flags)?;
        }
    }
    if let Some(command) = command {
        if !command.spec().scope.contains(&Scope::Cli) {
            bail!(
                "`{}` only works inside a session: /{}",
                command.spec().name,
                command.spec().name
            );
        }
        let cx = Ctx {
            app: &app,
            scope: Scope::Cli,
        };
        let out = command.run(&cx, &words[1..]).await?.text;
        if !out.is_empty() {
            term::out(&out);
            if !out.ends_with('\n') {
                term::out("\n");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    let prompt = words.join(" ");
    // A person at a terminal gets the session, with any words already typed in for review.
    if session {
        let initial = (!prompt.trim().is_empty()).then_some(prompt);
        return tui::run(&app, initial).await;
    }

    let mut prompt = prompt;
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
    let format = if cli.json { Format::Json } else { Format::Text };
    print::run(&app, prompt, format).await
}

/// Shows what the project's config would loosen and asks whether to trust the folder.
fn confirm_trust(untrusted: &Untrusted) -> anyhow::Result<bool> {
    term::out(&trust::warning(&untrusted.file, &untrusted.keys));
    term::out(trust::question());
    let answer = term::read_line()?.unwrap_or_default();
    let yes = matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
    if !yes {
        term::out(&format!("{}\n", trust::declined()));
    }
    Ok(yes)
}
