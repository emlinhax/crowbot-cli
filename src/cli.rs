use std::process::ExitCode;

use anyhow::anyhow;
use clap::{Parser, Subcommand};

use crate::app::App;
use crate::commands;
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

    #[command(subcommand)]
    command: Option<Sub>,
}

#[derive(Subcommand)]
enum Sub {
    /// Any command from the registry, e.g. `crowbot models`.
    #[command(external_subcommand)]
    Registry(Vec<String>),
}

pub async fn run() -> ExitCode {
    match dispatch(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            term::err(&format!("crowbot: {e:#}\n"));
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let app = App::load(&Overrides { model: cli.model })?;
    let args = match cli.command {
        Some(Sub::Registry(args)) => args,
        None => vec!["help".to_owned()],
    };
    let (name, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("missing command"))?;
    let command = commands::find(name)
        .ok_or_else(|| anyhow!("unknown command `{name}`; see `crowbot help`"))?;
    let out = command.run(&app, rest).await?;
    if !out.is_empty() {
        term::out(&out);
        if !out.ends_with('\n') {
            term::out("\n");
        }
    }
    Ok(())
}
