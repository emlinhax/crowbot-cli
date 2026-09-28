use std::process::ExitCode;

use anyhow::{anyhow, bail};
use clap::{Parser, Subcommand};

use crate::app::App;
use crate::commands;
use crate::frontend::print::{self, Format};
use crate::io::term;
use crate::settings::Overrides;

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
    let app = App::load(&Overrides {
        model: cli.model,
        effort: cli.effort,
    })?;
    let words = match cli.command {
        Some(Sub::Words(words)) => words,
        None => Vec::new(),
    };
    let command = words.first().and_then(|w| commands::find(w));
    if let (Some(command), false, false) = (command, cli.print, cli.json) {
        let out = command.run(&app, &words[1..]).await?;
        if !out.is_empty() {
            term::out(&out);
            if !out.ends_with('\n') {
                term::out("\n");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut prompt = words.join(" ");
    if let Some(piped) = term::piped_stdin()? {
        prompt = if prompt.is_empty() {
            piped
        } else {
            format!("{prompt}\n\n{piped}")
        };
    }
    if prompt.trim().is_empty() {
        if cli.print || cli.json {
            bail!("nothing to send; pass a prompt or pipe one in");
        }
        let help = commands::find("help").ok_or_else(|| anyhow!("help is registered"))?;
        term::out(&help.run(&app, &[]).await?);
        return Ok(ExitCode::SUCCESS);
    }
    let format = if cli.json { Format::Json } else { Format::Text };
    print::run(&app, prompt, format).await
}
