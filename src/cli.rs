use std::process::ExitCode;

use anyhow::bail;
use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::app::App;
use crate::commands::{self, Ctx, Missing, Scope};
use crate::frontend::print::{self, Format};
use crate::io::term;
use crate::settings::{Overrides, Untrusted};
use crate::{effort, mode, trust, tui};

/// The help for `--effort`, `--mode` and the commands is filled in from their catalogs by
/// `command()`, so a new level, mode or command shows up with no edit here.
#[derive(Parser)]
#[command(
    version,
    about,
    allow_external_subcommands = true,
    subcommand_value_name = "COMMAND|PROMPT"
)]
struct Cli {
    /// Model id for this run (see `crowbot models`).
    #[arg(long, global = true)]
    model: Option<String>,

    #[arg(long, global = true)]
    effort: Option<String>,

    #[arg(long, global = true)]
    mode: Option<String>,

    /// Send the words as a prompt, never a command; print the reply, exit.
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

/// `Cli`'s shape with the parts that are data filled in.
fn command() -> clap::Command {
    let ids = |ids: Vec<&str>| ids.join(", ");
    let efforts = ids(effort::levels().iter().map(|l| l.id.as_str()).collect());
    let modes = ids(mode::all().iter().map(|m| m.id.as_str()).collect());
    let names: Vec<&str> = commands::available(Scope::Cli)
        .map(|c| c.spec())
        .filter(|s| !s.hidden)
        .map(|s| s.name.as_str())
        .collect();
    Cli::command()
        .mut_arg("effort", |a| {
            a.help(format!("Reasoning effort for this run: {efforts}"))
        })
        .mut_arg("mode", |a| {
            a.help(format!("Permission mode for this run: {modes}"))
        })
        .after_help(format!(
            "Commands: {}. Other words are the prompt.",
            ids(names)
        ))
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
                format!("`{flag}` goes before the words; quote a prompt that contains it"),
            )
            .exit();
    }
    match dispatch(cli).await {
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
    let command = match words.first().filter(|_| !headless) {
        Some(name) => match commands::lookup(name, Scope::Cli) {
            Ok(command) => Some(command),
            Err(Missing::Elsewhere(spec)) => {
                bail!("`{}` only works as {}", spec.name, commands::places(spec))
            }
            Err(Missing::Unknown) => None,
        },
        None => None,
    };
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

/// One of crowbot's own flags after the words, where it would silently become prompt text (or
/// a command's argument) instead of taking effect.
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
