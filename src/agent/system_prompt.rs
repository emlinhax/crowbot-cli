//! The system prompt, ordered stable-first so vendors that cache prompt prefixes can reuse it.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::models::Model;
use crate::paths::Paths;
use crate::text::template;
use crate::{io, limits};

const BASE: &str = include_str!("../../data/prompts/system.md");
const ENV: &str = include_str!("../../data/prompts/env.md");

static PROJECT: LazyLock<Project> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/prompts/project.toml"))
        .expect("data/prompts/project.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    files: Vec<String>,
    frame: String,
    cut: String,
}

/// Reminders a mode can name in data/modes/*.toml; one line per prompt file.
const REMINDERS: &[(&str, &str)] =
    &[("plan_mode", include_str!("../../data/prompts/plan_mode.md"))];

pub fn build(paths: &Paths, model: &Model) -> String {
    let cwd = paths.project.display().to_string();
    let date = io::clock::today();
    let env = template::fill(
        ENV,
        &[
            ("cwd", &cwd),
            ("platform", std::env::consts::OS),
            ("date", &date),
            ("model", &model.id),
        ],
    );
    let parts = [
        Some(BASE.trim_end().to_owned()),
        project(paths),
        Some(env.trim_end().to_owned()),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
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
        let end = (0..=max)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        text.truncate(end);
        let cut = template::fill(&PROJECT.cut, &[("file", file), ("max", &max.to_string())]);
        text = format!("{text}\n{cut}");
    }
    Some(
        template::fill(&PROJECT.frame, &[("file", file), ("text", &text)])
            .trim()
            .to_owned(),
    )
}

pub fn reminder(name: &str) -> Option<&'static str> {
    REMINDERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| text.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Capabilities, Pricing};

    fn model() -> Model {
        Model {
            id: "m".into(),
            display_name: String::new(),
            description: String::new(),
            context_window: 1000,
            max_output_tokens: 100,
            pricing: Pricing {
                input_per_1m_usd: 1.0,
                cached_input_per_1m_usd: 0.1,
                output_per_1m_usd: 2.0,
            },
            capabilities: Capabilities::default(),
        }
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
        assert!(!build(&paths, &model()).contains("<project-instructions"));
        write("CLAUDE.md", "Use tabs.");
        assert!(build(&paths, &model()).contains("file=\"CLAUDE.md\">\nUse tabs."));
        write("AGENTS.md", "Run make test.");
        let prompt = build(&paths, &model());
        assert!(
            prompt.contains("Run make test.") && !prompt.contains("Use tabs."),
            "{prompt}"
        );
        assert!(prompt.find("Run make test.") < prompt.find("<env>"));
        let max = limits::get().agent.instructions_bytes.value;
        write("AGENTS.md", &"é".repeat(max));
        let prompt = build(&paths, &model());
        assert!(prompt.contains("is left out"), "the cut is not said");
        assert!(prompt.len() < BASE.len() + max + 1000);
    }
}
