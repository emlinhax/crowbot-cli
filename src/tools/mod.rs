//! Tools the model can call. Each is one file implementing `Tool`, registered once below.

mod bash;
mod codesearch;
mod edit;
pub mod forum;
mod fs;
mod glob;
mod grep;
mod many;
mod plan_exit;
mod question;
mod read;
mod target;
#[cfg(test)]
pub mod testing;
mod todowrite;
mod webfetch;
mod write;

pub use target::plan_file;

use std::sync::{Arc, LazyLock};

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::limits;
use crate::data;
use crate::live::event::AgentEvent;
use crate::live::files::Files;
use crate::live::prompt::{self, Prompt, Reply};
use crate::live::state::Shared;
use crate::permission::gate::Ask;
use crate::text::template;
use crate::text::truncate::{self, Keep};

/// A `LazyLock` of tool `name`'s spec: data/tools/<name>.md and <name>.schema.json.
macro_rules! spec {
    ($name:literal) => {
        std::sync::LazyLock::new(|| {
            $crate::tools::Spec::load(
                $name,
                $crate::data::raw!("tools/", $name, ".md"),
                $crate::data::raw!("tools/", $name, ".schema.json"),
            )
        })
    };
}
pub(crate) use spec;

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

static TEXT: LazyLock<Text> = data::load!("tools/output.toml");

/// What any tool says about its own output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub cut: String,
    pub item_failed: String,
    pub cancelled: String,
}

pub fn text() -> &'static Text {
    &TEXT
}

/// What a call needs before it may run, and what it would do, for the permission prompt.
pub struct Check {
    pub asks: Vec<Ask>,
    pub preview: Option<String>,
    /// The user confirms this call every time, even where rules and mode allow it; a deny still
    /// wins.
    pub confirm: bool,
}

impl Check {
    pub fn new(asks: Vec<Ask>) -> Self {
        Self {
            asks,
            preview: None,
            confirm: false,
        }
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    pub fn confirmed(mut self) -> Self {
        self.confirm = true;
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

impl From<String> for Refusal {
    fn from(why: String) -> Self {
        Self::Refused(why)
    }
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

    /// `text` from its head, cut to the tool limits, with a note saying so when it was.
    pub fn head(text: &str) -> Self {
        let limits = &limits::get().tools;
        let cut = truncate::cut(
            text,
            Keep::Head,
            limits.max_lines.value,
            limits.max_bytes.value,
        );
        let mut content = cut.text.clone();
        if cut.truncated() {
            content.push_str("\n\n");
            content.push_str(&template::fill(
                &TEXT.cut,
                &[
                    ("kept", &cut.kept_lines.to_string()),
                    ("total", &cut.total_lines.to_string()),
                ],
            ));
        }
        Self::ok(content)
    }

    /// The call was interrupted before it finished.
    pub fn cancelled() -> Self {
        Self::error(TEXT.cancelled.clone())
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
    /// The call's arguments, parsed from the model's JSON before `check` and `run` see them.
    type Args: DeserializeOwned + Send;
    fn spec(&self) -> &Spec;
    /// Validates the call and says what it needs permission for; runs before any prompt.
    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal>;
    /// An `Err` reaches the model as an error output.
    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>>;
}

/// A `Tool` that takes the model's JSON, so tools with different arguments share one registry.
pub trait AnyTool: Send + Sync {
    fn spec(&self) -> &Spec;
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal>;
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output>;
}

impl<T: Tool> AnyTool for T {
    fn spec(&self) -> &Spec {
        Tool::spec(self)
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args = T::Args::deserialize(args).map_err(|e| Refusal::InvalidArgs(e.to_string()))?;
        Tool::check(self, &args, cx)
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            // `check` has vetted the arguments, so this only fails when a caller skipped it.
            let done = match serde_json::from_value(args) {
                Ok(args) => Tool::run(self, args, cx).await,
                Err(e) => Err(e.to_string()),
            };
            done.unwrap_or_else(Output::error)
        })
    }
}

pub struct Registry {
    tools: Vec<Arc<dyn AnyTool>>,
    /// Values for `{placeholders}` in tool descriptions: every `tools.*` limit, and the shell.
    vars: Vec<(String, String)>,
}

impl Registry {
    /// Every tool, with bash running `shell` (the `shell` setting; `None` picks one).
    pub fn builtin(shell: Option<&str>) -> Self {
        let shell = bash::shell::resolve(shell);
        let mut vars = limits::vars("tools");
        vars.push(("shell_note".to_owned(), shell.note.clone()));
        let tools: Vec<Arc<dyn AnyTool>> = vec![
            Arc::new(read::Read),
            Arc::new(write::Write),
            Arc::new(edit::Edit),
            Arc::new(fs::Fs),
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

    pub fn get(&self, name: &str) -> Option<&Arc<dyn AnyTool>> {
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
        let vars: Vec<(&str, &str)> = self
            .vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
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
mod tests {
    use super::testing::{Project, sample};
    use super::*;

    #[test]
    fn every_schema_fits_its_arguments() {
        let project = Project::new();
        let registry = Registry::builtin(None);
        for tool in &registry.tools {
            let args = sample(&tool.spec().parameters);
            if let Err(Refusal::InvalidArgs(e)) = project.check(tool.as_ref(), &args) {
                panic!("{}: schema sample rejected: {e}", tool.spec().name);
            }
        }
    }

    #[test]
    fn a_long_reply_keeps_its_head_and_says_where_it_was_cut() {
        let max = limits::get().tools.max_lines.value;
        let text: String = (0..max + 5).map(|i| format!("{i}\n")).collect();
        let out = Output::head(&text);
        assert!(out.content.starts_with("0\n1\n"), "{}", out.content);
        let note = format!("[Cut at {max} of {} lines.]", max + 5);
        assert!(out.content.ends_with(&note), "{}", out.content);
        assert_eq!(Output::head("short").content, "short");
    }

    #[test]
    fn wire_specs_fill_every_placeholder() {
        for spec in Registry::builtin(None).wire() {
            let text = spec["function"]["description"].as_str().unwrap();
            assert!(!text.contains('{'), "unfilled placeholder in: {text}");
        }
    }
}
