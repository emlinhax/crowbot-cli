use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::LazyLock;

use anyhow::bail;
use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Deserialize;

use crate::app::App;
use crate::commands::{self, Ctx, Missing, Scope};
use crate::config::settings::{self, Overrides};
use crate::config::trust;
use crate::data;
use crate::frontend::print::{self, Format};
use crate::frontend::tui;
use crate::io::term;
use crate::release::{self, install};
use crate::session;
use crate::text::template::fill;
use crate::{effort, mode};

static DATA: LazyLock<Data> = data::load!("cli.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    /// Each flag's help, by arg id; `command()` fills it in.
    help: BTreeMap<String, String>,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    commands: String,
    misplaced_flag: String,
    no_session: String,
}

/// The flags' help and the commands' list are data (data/cli.toml), filled in by `command()`.
#[derive(Parser)]
#[command(
    about,
    allow_external_subcommands = true,
    subcommand_value_name = "COMMAND|PROMPT"
)]
struct Cli {
    #[command(flatten)]
    flags: Overrides,

    #[arg(short, long)]
    print: bool,

    #[arg(long)]
    json: bool,

    #[arg(short = 'c', long = "continue", id = "continue")]
    resume: bool,

    #[command(subcommand)]
    command: Option<Sub>,
}

#[derive(Subcommand)]
enum Sub {
    /// A command from the registry (`crowbot help`), or else the words of a prompt.
    #[command(external_subcommand)]
    Words(Vec<String>),
}

/// `Cli`'s shape with the parts that are data filled in.
fn command() -> clap::Command {
    let names: Vec<&str> = commands::listed(Scope::Cli)
        .map(|s| s.name.as_str())
        .collect();
    let vars = [
        ("levels", effort::ids().join(", ")),
        ("modes", mode::ids().join(", ")),
        ("commands", names.join(", ")),
    ];
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut command = Cli::command()
        .version(release::version())
        .after_help(fill(&DATA.text.commands, &vars));
    for (id, help) in &DATA.help {
        command = command.mut_arg(id, |a| a.help(fill(help, &vars)));
    }
    command
}

fn parse() -> Cli {
    Cli::from_arg_matches(&command().get_matches()).unwrap_or_else(|e| e.exit())
}

pub async fn run() -> ExitCode {
    let cli = parse();
    if let Some(Sub::Words(words)) = &cli.command
        && let Some(flag) = misplaced_flag(words)
    {
        command()
            .error(
                ErrorKind::ArgumentConflict,
                fill(&DATA.text.misplaced_flag, &[("flag", flag)]),
            )
            .exit();
    }
    match dispatch(cli).await {
        Ok(code) => code,
        Err(e) => {
            term::warn(&format!("{e:#}"));
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<ExitCode> {
    let flags = cli.flags;
    let mut app = App::load(&flags)?;
    let words = match cli.command {
        Some(Sub::Words(words)) => words,
        None => Vec::new(),
    };
    let headless = cli.print || cli.json;
    let command = match words.first().filter(|_| !headless) {
        Some(name) => match commands::lookup(name, Scope::Cli) {
            Ok(command) => Some(command),
            Err(Missing::Elsewhere(spec)) => {
                bail!(commands::elsewhere(spec, Scope::Cli))
            }
            Err(Missing::Unknown) => None,
        },
        None => None,
    };
    let session =
        command.is_none() && !headless && term::stdin_is_terminal() && term::stdout_is_terminal();
    if session {
        install::offer(&app.paths)?;
    }
    let agents = crate::agent::kinds::left_out(&app.paths);
    let untrusted = trust::with_agents(&app.paths, app.settings.untrusted.take(), &agents);
    if let Some(untrusted) = untrusted
        && trust::settle(&app.paths, &untrusted, session)?
    {
        // Trusted now: the project's agent files count too.
        app.agent_notes = crate::agent::kinds::init(&app.paths);
        app.settings = settings::load(&app.paths, &flags)?;
    }
    for note in std::mem::take(&mut app.agent_notes) {
        term::warn(&note);
    }
    // Only a run starts servers, so only a run looks for those other apps list.
    if command.is_none() {
        crate::mcp::discover::choose::settle(&app.paths, &mut app.settings, session)?;
    }
    if let Some(command) = command {
        let cx = Ctx {
            session: None,
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

    let resume = match cli.resume {
        true => Some(session::list::latest(&app.paths).ok_or_else(|| {
            let project = app.paths.project.display().to_string();
            anyhow::anyhow!(fill(&DATA.text.no_session, &[("project", &project)]))
        })?),
        false => None,
    };
    let prompt = words.join(" ");
    // At a terminal: the session, with the words typed in for review.
    if session {
        let initial = (!prompt.trim().is_empty()).then_some(prompt);
        return tui::run(&app, initial, resume.as_deref()).await;
    }

    let format = if cli.json { Format::Json } else { Format::Text };
    print::run(&app, prompt, format, resume.as_deref()).await
}

/// A crowbot flag after the words, which would otherwise become prompt text or a command argument.
fn misplaced_flag(words: &[String]) -> Option<&str> {
    let command = command();
    let ours = |word: &str| {
        if let Some(long) = word.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            command.get_arguments().any(|a| a.get_long() == Some(name))
        } else if let Some(short) = word.strip_prefix('-') {
            let mut chars = short.chars();
            matches!((chars.next(), chars.next()), (Some(c), None)
                if command.get_arguments().any(|a| a.get_short() == Some(c)))
        } else {
            false
        }
    };
    words.iter().map(String::as_str).find(|w| ours(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_built_from_the_catalogs() {
        command().debug_assert();
        let help = command().render_long_help().to_string();
        for name in ["help", "login", "logout", "signup", "models"] {
            assert!(help.contains(name), "{name} missing from:\n{help}");
        }
        assert!(!help.contains("keytest"), "hidden commands stay hidden");
        for level in effort::levels() {
            assert!(help.contains(&level.id), "{}", level.id);
        }
        for mode in mode::all() {
            assert!(help.contains(&mode.id), "{}", mode.id);
        }
    }

    #[test]
    fn crowbots_own_flags_after_the_words_are_found() {
        let found = |words: &[&str]| {
            let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
            misplaced_flag(&words).map(str::to_owned)
        };
        assert_eq!(found(&["hi", "--json"]).as_deref(), Some("--json"));
        assert_eq!(found(&["hi", "--model=x"]).as_deref(), Some("--model=x"));
        assert_eq!(found(&["hi", "-p"]).as_deref(), Some("-p"));
        assert_eq!(found(&["login", "--key", "1"]), None);
        assert_eq!(found(&["login", "--help"]), None);
        assert_eq!(found(&["explain --json"]), None);
    }
}
