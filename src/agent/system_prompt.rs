//! The system prompt, ordered stable-first so vendors that cache prompt prefixes can reuse it.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::config::limits;
use crate::config::paths::Paths;
use crate::data;
use crate::io;
use crate::mcp::Connected;
use crate::text::template;

const BASE: &str = data::raw!("prompts/system.md");
const ENV: &str = data::raw!("prompts/env.md");

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
/// session's MCP servers.
pub fn build(paths: &Paths, model: &str, servers: &[Connected]) -> String {
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
    let parts = [
        Some(BASE.trim_end().to_owned()),
        forums(paths),
        mcp(servers),
        project(paths),
        Some(env.trim_end().to_owned()),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
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
        let prompt = build(&paths, "m", &servers);
        let named = prompt.find("The user connected these MCP servers: ida, long, quiet.");
        let ida = prompt.find("<mcp-server-instructions server=\"ida\">\nOpen a database first.");
        let project = prompt.find("<project-instructions");
        assert!(named.is_some() && named < ida && ida < project, "{prompt}");
        assert!(prompt.contains("never from memory"), "{prompt}");
        assert!(!prompt.contains("server=\"quiet\""), "{prompt}");
        assert!(prompt.contains("they run past"), "{prompt}");
        assert!(!prompt.contains(&"x".repeat(max + 1)));
        assert!(!build(&paths, "m", &[]).contains("<mcp-servers>"));
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
        let bare = build(&paths, "m", &[]);
        assert!(!bare.contains("<project-instructions"));
        write("CLAUDE.md", "Use tabs.");
        assert!(build(&paths, "m", &[]).contains("file=\"CLAUDE.md\">\nUse tabs."));
        write("AGENTS.md", "Run make test.");
        let prompt = build(&paths, "m", &[]);
        assert!(
            prompt.contains("Run make test.") && !prompt.contains("Use tabs."),
            "{prompt}"
        );
        assert!(prompt.find("Run make test.") < prompt.find("<env>"));
        let max = limits::get().agent.instructions_bytes.value;
        write("AGENTS.md", &"é".repeat(max));
        let prompt = build(&paths, "m", &[]);
        assert!(prompt.contains("is left out"), "the cut is not said");
        assert!(prompt.len() < bare.len() + max + 1000);
    }

    #[test]
    fn the_forums_to_browse_are_named_with_their_hints() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        // With no store yet, the seed is what there is to browse.
        let seed = &crate::forums::data().seed[0];
        let prompt = build(&paths, "m", &[]);
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
        assert!(!build(&paths, "m", &[]).contains("<forums>"));
    }
}
