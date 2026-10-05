//! Tools the model can call. Each is one file implementing `Tool`, registered once below.

mod bash;
mod codesearch;
mod edit;
mod edit_match;
pub mod files;
mod forum;
mod glob;
mod grep;
mod many;
pub mod permissions;
mod plan_exit;
mod question;
mod read;
mod shell;
mod target;
mod todowrite;
mod truncate;
mod webfetch;
mod write;

use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::agent::event::AgentEvent;
use crate::agent::prompt::{self, Prompt, Reply};
use crate::agent::state::Shared;
use crate::app::App;
use crate::limits;
use crate::paths::Paths;
use crate::permission::gate::Ask;
use crate::text::template;
use files::Files;

/// A tool's name, description (data/tools/<name>.md) and argument schema (<name>.schema.json).
pub struct Spec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

impl Spec {
    pub fn load(name: &'static str, description: &'static str, schema: &str) -> Self {
        Self {
            name,
            description,
            parameters: serde_json::from_str(schema).expect("tool schemas are checked by tests"),
        }
    }
}

/// What a call needs before it may run, and what it would do, for the permission prompt.
pub struct Check {
    pub asks: Vec<Ask>,
    pub preview: Option<String>,
}

impl Check {
    pub fn new(asks: Vec<Ask>) -> Self {
        Self {
            asks,
            preview: None,
        }
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    /// The arguments do not fit the schema; the model is told so once, in batch.
    #[error("{0}")]
    InvalidArgs(String),
    /// Well-formed, but cannot be done (e.g. a missing file).
    #[error("{0}")]
    Refused(String),
}

pub struct Output {
    pub content: String,
    pub is_error: bool,
    pub details: Option<Value>,
}

impl Output {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

pub struct ToolCx<'a> {
    pub app: &'a App,
    /// The session: what the model has read, the mode, and the way to ask the user.
    pub shared: &'a Shared,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
    pub call_id: &'a str,
    /// The only file PLAN mode may write.
    pub plan_file: &'a str,
    /// Fires when the user interrupts; long-running tools stop on it.
    pub cancel: tokio_util::sync::CancellationToken,
}

impl ToolCx<'_> {
    pub fn files(&self) -> &Files {
        &self.shared.files
    }

    /// Puts a question to the user and waits; `None` when the run was interrupted.
    pub async fn ask(&self, prompt: Prompt) -> Option<Reply> {
        prompt::ask(self.shared, self.emit, self.call_id, prompt, &self.cancel).await
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> &Spec;
    /// Validates the call and says what it needs permission for; runs before any prompt.
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal>;
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output>;
}

pub fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, Refusal> {
    serde_json::from_value(args.clone()).map_err(|e| Refusal::InvalidArgs(e.to_string()))
}

/// `parse` for `run`, where `check` has already vetted the arguments.
pub fn parse_or_fail<T: DeserializeOwned>(args: &Value) -> Result<T, Output> {
    parse(args).map_err(|e| Output::error(e.to_string()))
}

/// Where PLAN writes this session's plan, spelled as the tools resolve it, so the lock that lets
/// PLAN write there matches what the write tool asks for.
pub fn plan_file(paths: &Paths, session: &str) -> String {
    let file = paths.plans_dir().join(format!("{session}.md"));
    let file = file.to_string_lossy();
    target::resolve(paths, &file).map_or_else(|_| file.replace('\\', "/"), |t| t.shown)
}

pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
    /// Values for `{placeholders}` in tool descriptions.
    vars: Vec<(&'static str, String)>,
}

impl Registry {
    pub fn builtin(app: &App) -> Self {
        let shell = shell::resolve(app.settings.shell.as_deref());
        let limits = &limits::get().tools;
        let vars = vec![
            ("shell_note", shell.note.clone()),
            ("timeout", limits.bash_timeout_secs.value.to_string()),
            (
                "max_timeout",
                limits.bash_max_timeout_secs.value.to_string(),
            ),
            ("max_lines", limits.max_lines.value.to_string()),
            ("max_results", limits.max_results.value.to_string()),
            (
                "grep_context_max",
                limits.grep_context_max.value.to_string(),
            ),
            ("read_batch_max", limits.read_batch_max.value.to_string()),
            ("list_depth_max", limits.list_depth_max.value.to_string()),
        ];
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(read::Read),
            Arc::new(write::Write),
            Arc::new(edit::Edit),
            Arc::new(bash::Bash::new(shell)),
            Arc::new(glob::Glob),
            Arc::new(grep::Grep),
            Arc::new(webfetch::WebFetch),
            Arc::new(forum::Sections),
            Arc::new(forum::Topics),
            Arc::new(forum::ThreadTool),
            Arc::new(forum::Search),
            Arc::new(forum::Post),
            Arc::new(codesearch::CodeSearch),
            Arc::new(todowrite::TodoWrite),
            Arc::new(question::Question),
            Arc::new(plan_exit::PlanExit),
        ];
        Self { tools, vars }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.spec().name == name)
    }

    pub fn names(&self) -> String {
        self.tools
            .iter()
            .map(|t| t.spec().name)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The `tools` array of a chat request.
    pub fn wire(&self) -> Vec<Value> {
        let vars: Vec<(&str, &str)> = self.vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.tools
            .iter()
            .map(|t| {
                let spec = t.spec();
                json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": template::fill(spec.description, &vars).trim(),
                        "parameters": spec.parameters,
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
pub mod testing {
    //! A throwaway project and the context tools run in.

    use super::*;
    use crate::app::App;

    pub struct Project {
        /// Held so the directory lives as long as the test.
        _dir: tempfile::TempDir,
        pub app: App,
        pub shared: Shared,
    }

    impl Project {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let app = App::for_tests(dir.path());
            Self {
                _dir: dir,
                app,
                shared: Shared::new(crate::mode::get("manual").unwrap()),
            }
        }

        pub fn cx(&self) -> ToolCx<'_> {
            ToolCx {
                app: &self.app,
                shared: &self.shared,
                emit: &|_| {},
                call_id: "test",
                plan_file: "/nowhere/plan.md",
                cancel: tokio_util::sync::CancellationToken::new(),
            }
        }

        pub fn write(&self, path: &str, text: &str) {
            crate::io::fs::write_atomic(
                &self.app.paths.project.join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }

        pub fn read(&self, path: &str) -> String {
            crate::io::fs::read_string(&self.app.paths.project.join(path))
                .unwrap()
                .unwrap()
        }
    }

    /// A sample value for every property in `schema`, required or not.
    pub fn sample(schema: &Value) -> Value {
        match schema["type"].as_str() {
            Some("object") => {
                let props = schema["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                Value::Object(props.iter().map(|(k, v)| (k.clone(), sample(v))).collect())
            }
            Some("array") => json!([sample(&schema["items"])]),
            Some("integer") => json!(1),
            Some("boolean") => json!(true),
            _ => match schema["enum"].as_array() {
                Some(choices) => choices[0].clone(),
                None => json!("x"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Project, sample};
    use super::*;

    #[test]
    fn every_schema_fits_its_arguments() {
        let project = Project::new();
        let registry = Registry::builtin(&project.app);
        for tool in &registry.tools {
            let args = sample(&tool.spec().parameters);
            if let Err(Refusal::InvalidArgs(e)) = tool.check(&args, &project.cx()) {
                panic!("{}: schema sample rejected: {e}", tool.spec().name);
            }
        }
    }

    #[test]
    fn wire_specs_fill_every_placeholder() {
        let project = Project::new();
        for spec in Registry::builtin(&project.app).wire() {
            let text = spec["function"]["description"].as_str().unwrap();
            assert!(!text.contains('{'), "unfilled placeholder in: {text}");
        }
    }
}
