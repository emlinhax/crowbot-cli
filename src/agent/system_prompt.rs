//! The system prompt, ordered stable-first so vendors that cache prompt prefixes can reuse it.

use crate::api::models::Model;
use crate::io;
use crate::paths::Paths;
use crate::text::template;

const BASE: &str = include_str!("../../data/prompts/system.md");
const ENV: &str = include_str!("../../data/prompts/env.md");

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
    format!("{}\n\n{}", BASE.trim_end(), env.trim_end())
}

pub fn reminder(name: &str) -> Option<&'static str> {
    REMINDERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| text.trim_end())
}
