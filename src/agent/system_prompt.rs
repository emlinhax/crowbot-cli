//! The system prompt, ordered stable-first so vendors that cache prompt prefixes can reuse it.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::{kinds, roles};
use crate::config::limits;
use crate::config::paths::Paths;
use crate::config::subagents::Subagents;
use crate::data;
use crate::io;
use crate::mcp::Connected;
use crate::models::Catalog;
use crate::text::{template, units};

const BASE: &str = data::raw!("prompts/system.md");
const ENV: &str = data::raw!("prompts/env.md");
/// For a run nobody can answer: say so, so the model acts instead of asking.
const HEADLESS: &str = data::raw!("prompts/headless.md");

static PROJECT: LazyLock<Project> = data::load!("prompts/project.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    files: Vec<String>,
    frame: String,
    cut: String,
}

static FORUMS: LazyLock<Forums> = data::load!("prompts/forums.toml");
static MCP: LazyLock<Mcp> = data::load!("prompts/mcp.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mcp {
    servers: String,
    frame: String,
    cut: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Forums {
    frame: String,
}

/// Reminders a mode can name in data/modes/*.toml; one line per prompt file. `servers` are the
/// session's MCP servers; `headless` is a run nobody can answer (`-p`, `--json`); `subagents` is
/// `subagent_models`' text.
pub fn build(
    paths: &Paths,
    model: &str,
    servers: &[Connected],
    headless: bool,
    subagents: &str,
) -> String {
    let env = env(paths, model);
    let parts = [
        Some(BASE.trim_end().to_owned()),
        forums(paths),
        mcp(servers),
        project(paths),
        Some(subagents.to_owned()).filter(|s| !s.is_empty()),
        headless.then(|| HEADLESS.trim_end().to_owned()),
        Some(env),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
}

/// The environment block: where, when, and on which model.
pub fn env(paths: &Paths, model: &str) -> String {
    let cwd = paths.project.display().to_string();
    let date = io::clock::today();
    let env = template::fill(
        ENV,
        &[
            ("cwd", &cwd),
            ("platform", std::env::consts::OS),
            ("date", &date),
            ("model", model),
        ],
    );
    env.trim_end().to_owned()
}

static SUBAGENTS: LazyLock<SubagentModels> = data::load!("prompts/subagents.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubagentModels {
    frame: String,
    kind: String,
    model: String,
}

/// What each kind of subagent runs on and what a `task` call may name instead, for the system
/// prompt; `chat` is the model the session uses.
pub fn subagent_models(catalog: &Catalog, subagents: &Subagents, chat: &str) -> String {
    let words = &*SUBAGENTS;
    let kinds: Vec<String> = kinds::all()
        .iter()
        .map(|k| {
            let (model, _) = roles::chosen(Some(&k.id), None, subagents, chat);
            template::fill(&words.kind, &[("kind", &k.id), ("model", model)])
        })
        .collect();
    let allowed = |id: &str| {
        subagents
            .choices
            .as_ref()
            .is_none_or(|c| c.iter().any(|m| m == id))
    };
    let models: Vec<String> = catalog
        .models
        .iter()
        .filter(|m| allowed(&m.id))
        .map(|m| {
            template::fill(
                &words.model,
                &[
                    ("id", &m.id),
                    ("input", &units::usd(m.pricing.input_per_1m_usd)),
                    ("output", &units::usd(m.pricing.output_per_1m_usd)),
                    ("context", &units::tokens(m.context_window)),
                ],
            )
        })
        .collect();
    template::fill(
        &words.frame,
        &[("kinds", &kinds.join("\n")), ("models", &models.join("\n"))],
    )
    .trim()
    .to_owned()
}

/// `system`, built for `from`, as told to a run on `to`: only the env block's model line changes.
pub fn retarget(system: &str, from: &str, to: &str) -> String {
    let Some(line) = ENV.lines().find(|l| l.contains("{model}")) else {
        return system.to_owned();
    };
    let old = template::fill(line, &[("model", from)]);
    match system.rfind(&old) {
        Some(at) if from != to => {
            let new = template::fill(line, &[("model", to)]);
            format!("{}{new}{}", &system[..at], &system[at + old.len()..])
        }
        _ => system.to_owned(),
    }
}

/// The forums this machine can browse, each with its hint, so the model knows to look there.
fn forums(paths: &Paths) -> Option<String> {
    // An unreadable store is reported where forums are used; the prompt just goes without.
    let list = crate::forums::store::load(paths).ok()?;
    if list.is_empty() {
        return None;
    }
    let lines = crate::tools::forum::list(&list, false);
    Some(
        template::fill(&FORUMS.frame, &[("forums", &lines)])
            .trim()
            .to_owned(),
    )
}

/// The session's MCP servers by name, then each one's instructions, framed, each within
/// `mcp.instructions_chars`.
fn mcp(servers: &[Connected]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    let listed = template::fill(&MCP.servers, &[("names", &names.join(", "))]);
    let max = limits::get().mcp.instructions_chars.value;
    let framed: Vec<String> = servers
        .iter()
        .filter_map(|s| Some((&s.name, s.instructions.as_deref()?)))
        .map(|(server, text)| {
            let text = text.trim();
            let mut kept: String = text.chars().take(max).collect();
            if kept.len() < text.len() {
                let cut = template::fill(&MCP.cut, &[("max", &max.to_string())]);
                kept = format!("{}\n{cut}", kept.trim_end());
            }
            template::fill(&MCP.frame, &[("server", server), ("text", &kept)])
                .trim()
                .to_owned()
        })
        .collect();
    let mut parts = vec![listed.trim().to_owned()];
    parts.extend(framed);
    Some(parts.join("\n\n"))
}

/// The first instructions file the project has, framed, within `agent.instructions_bytes`.
fn project(paths: &Paths) -> Option<String> {
    let (file, text) = PROJECT.files.iter().find_map(|file| {
        let text = io::fs::read_string(&paths.project.join(file))
            .ok()
            .flatten()?;
        Some((file, text)).filter(|(_, t)| !t.trim().is_empty())
    })?;
    let max = limits::get().agent.instructions_bytes.value;
    let mut text = text.trim_end().to_owned();
    if text.len() > max {
        text.truncate(text.floor_char_boundary(max));
        let cut = template::fill(&PROJECT.cut, &[("file", file), ("max", &max.to_string())]);
        text = format!("{text}\n{cut}");
    }
    Some(
        template::fill(&PROJECT.frame, &[("file", file), ("text", &text)])
            .trim()
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_are_named_and_their_instructions_join_before_the_project_cut_to_fit() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        crate::io::fs::make_dir(&paths.project).unwrap();
        crate::io::fs::write_atomic(
            &paths.project.join("AGENTS.md"),
            b"Use tabs.",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let max = limits::get().mcp.instructions_chars.value;
        let server = |name: &str, instructions: Option<String>| Connected {
            name: name.into(),
            instructions,
        };
        let servers = [
            server("ida", Some("Open a database first.".into())),
            server("long", Some("x".repeat(max + 10))),
            server("quiet", None),
        ];
        let prompt = build(&paths, "m", &servers, false, "");
        let named = prompt.find("The user connected these MCP servers: ida, long, quiet.");
        let ida = prompt.find("<mcp-server-instructions server=\"ida\">\nOpen a database first.");
        let project = prompt.find("<project-instructions");
        assert!(named.is_some() && named < ida && ida < project, "{prompt}");
        assert!(prompt.contains("never from memory"), "{prompt}");
        assert!(!prompt.contains("server=\"quiet\""), "{prompt}");
        assert!(prompt.contains("they run past"), "{prompt}");
        assert!(!prompt.contains(&"x".repeat(max + 1)));
        assert!(!build(&paths, "m", &[], false, "").contains("<mcp-servers>"));
    }

    #[test]
    fn a_headless_run_is_told_nobody_will_answer() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        let headless = build(&paths, "m", &[], true, "");
        let at = headless.find("<headless>").unwrap();
        assert!(at < headless.find("<env>").unwrap(), "{headless}");
        assert!(!build(&paths, "m", &[], false, "").contains("<headless>"));
    }

    #[test]
    fn the_projects_instructions_join_the_prompt_before_the_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        let write = |name: &str, text: &str| {
            io::fs::write_atomic(
                &paths.project.join(name),
                text.as_bytes(),
                io::fs::Access::Shared,
            )
            .unwrap();
        };
        let bare = build(&paths, "m", &[], false, "");
        assert!(!bare.contains("<project-instructions"));
        write("CLAUDE.md", "Use tabs.");
        assert!(build(&paths, "m", &[], false, "").contains("file=\"CLAUDE.md\">\nUse tabs."));
        write("AGENTS.md", "Run make test.");
        let prompt = build(&paths, "m", &[], false, "");
        assert!(
            prompt.contains("Run make test.") && !prompt.contains("Use tabs."),
            "{prompt}"
        );
        assert!(prompt.find("Run make test.") < prompt.find("<env>"));
        let max = limits::get().agent.instructions_bytes.value;
        write("AGENTS.md", &"é".repeat(max));
        let prompt = build(&paths, "m", &[], false, "");
        assert!(prompt.contains("is left out"), "the cut is not said");
        assert!(prompt.len() < bare.len() + max + 1000);
    }

    #[test]
    fn the_forums_to_browse_are_named_with_their_hints() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        // With no store yet, the seed is what there is to browse.
        let seed = &crate::forums::data().seed[0];
        let prompt = build(&paths, "m", &[], false, "");
        assert!(
            prompt.contains(&format!(
                "- {} ({})\n  {}",
                seed.name,
                seed.host(),
                seed.hint
            )),
            "{prompt}"
        );
        assert!(prompt.find("<forums>") < prompt.find("<env>"));
        crate::forums::store::remove(&paths, seed).unwrap();
        assert!(!build(&paths, "m", &[], false, "").contains("<forums>"));
    }

    #[test]
    fn a_subagent_on_another_model_is_told_its_own_and_nothing_else_changes() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        let prompt = build(&paths, "chat-model", &[], false, "");
        let child = retarget(&prompt, "chat-model", "cheap-model");
        assert!(
            child.contains("Model: cheap-model") && !child.contains("chat-model"),
            "{child}"
        );
        assert_eq!(
            child.len() + "chat-model".len(),
            prompt.len() + "cheap-model".len()
        );
        assert_eq!(retarget(&prompt, "chat-model", "chat-model"), prompt);
    }

    #[test]
    fn the_subagent_models_section_names_each_kinds_model_and_only_the_allowed_choices() {
        let catalog = Catalog {
            models: crate::models::snapshot(),
            source: crate::models::Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        let (a, b) = (&catalog.models[0].id, &catalog.models[1].id);
        let mut subagents = Subagents {
            choices: Some(vec![a.clone()]),
            ..Subagents::default()
        };
        subagents.set(
            Some("explore"),
            crate::config::subagents::Role {
                model: Some(b.clone()),
                effort: None,
            },
        );
        let text = subagent_models(&catalog, &subagents, "chat-model");
        assert!(text.contains(&format!("- `explore`: {b}")), "{text}");
        assert!(text.contains("- `general`: chat-model"), "{text}");
        assert!(text.contains(&format!("- `{a}`: ")), "{text}");
        assert!(!text.contains(&format!("- `{b}`: ")), "{text}");
    }
}
