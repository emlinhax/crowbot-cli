//! Tools the model can call. Each is one file implementing `Tool`, registered once below.

mod bash;
mod claim;
mod decompile;
mod edit;
mod finish;
pub mod forum;
mod fs;
mod glob;
mod grep;
mod many;
pub mod mcp;
mod plan_exit;
mod question;
mod read;
mod target;
mod task;
#[cfg(test)]
pub mod testing;
mod todowrite;
mod webfetch;
mod write;

pub use bash::shell;
pub use claim::NAME as CLAIM;
pub use finish::NAME as FINISH;
pub use target::plan_file;
pub use task::NAME as TASK;

use std::sync::{Arc, LazyLock};

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::limits;
use crate::conversation::message::Usage;
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

/// A tool's name, description and argument schema: a built-in's from data/tools/<name>.md and
/// <name>.schema.json, or what another source (an MCP server) says of its own.
pub struct Spec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl Spec {
    pub fn load(name: &str, description: &str, schema: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
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
    /// What the call's own requests cost (a subagent's), recorded with its result.
    pub usage: Option<Usage>,
    /// The user declined something inside the call without a word, as at a prompt of the run's
    /// own: the run stops after this batch.
    pub stop: bool,
}

impl Output {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
            usage: None,
            stop: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
            usage: None,
            stop: false,
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

    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Stops the run after this batch, as a plain No at its own prompt does.
    pub fn stopping(mut self) -> Self {
        self.stop = true;
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
    /// The run making the call, for a tool that starts a run under it (`task`); `None` only where
    /// a test calls a tool on its own.
    pub run: Option<&'a crate::agent::Run<'a>>,
}

impl ToolCx<'_> {
    pub fn files(&self) -> &Files {
        &self.shared.files
    }

    /// Puts a question to the user and waits; `None` when the run was interrupted.
    pub async fn ask(&self, prompt: Prompt) -> Option<Reply> {
        prompt::ask(
            &self.shared.prompts,
            self.emit,
            self.call_id,
            prompt,
            &self.cancel,
        )
        .await
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
    /// Whether the tool ends on an interrupt by itself, so the run waits for it rather than
    /// dropping it: a subagent records its last turn and what it spent.
    fn stops_on_cancel(&self) -> bool {
        false
    }
}

/// A `Tool` that takes the model's JSON, so tools with different arguments share one registry.
pub trait AnyTool: Send + Sync {
    fn spec(&self) -> &Spec;
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal>;
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output>;
    fn stops_on_cancel(&self) -> bool;
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

    fn stops_on_cancel(&self) -> bool {
        Tool::stops_on_cancel(self)
    }
}

/// The tools a run offers, in the order the model is shown them.
pub struct Registry {
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    tool: Arc<dyn AnyTool>,
    /// The tool as a chat request lists it, built once when it joins, so every request sends the
    /// same bytes and the vendor's cache of them holds.
    wire: Value,
}

/// The tool's name is another's already; the first keeps it.
#[derive(Debug)]
pub struct Taken;

impl Registry {
    /// Every tool, with bash running `shell` (the `shell` setting; `None` picks one).
    pub fn builtin(shell: Option<&str>) -> Self {
        let shell = bash::shell::resolve(shell);
        let mut vars = limits::vars("tools");
        vars.push(("shell_note".to_owned(), shell.note.clone()));
        let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let tools: Vec<Arc<dyn AnyTool>> = vec![
            Arc::new(read::Read),
            Arc::new(write::Write),
            Arc::new(edit::Edit),
            Arc::new(fs::Fs),
            Arc::new(bash::Bash::new(shell)),
            Arc::new(glob::Glob),
            Arc::new(grep::Grep),
            Arc::new(decompile::Decompile),
            Arc::new(webfetch::WebFetch),
            Arc::new(forum::Sections),
            Arc::new(forum::Topics),
            Arc::new(forum::ThreadTool),
            Arc::new(forum::Search),
            Arc::new(forum::Post),
            Arc::new(todowrite::TodoWrite),
            Arc::new(question::Question),
            Arc::new(plan_exit::PlanExit),
            Arc::new(task::Task::new()),
            Arc::new(claim::Claim),
            Arc::new(finish::Finish),
        ];
        let mut registry = Self {
            entries: Vec::new(),
        };
        for tool in tools {
            registry
                .add(tool, &vars)
                .expect("built-in tools have names of their own");
        }
        registry
    }

    /// Adds `tool` after the others, its description's `{placeholders}` filled from `vars`; a
    /// tool from outside passes none, so its words go out as written.
    pub fn add(&mut self, tool: Arc<dyn AnyTool>, vars: &[(&str, &str)]) -> Result<(), Taken> {
        let spec = tool.spec();
        if self.get(&spec.name).is_some() {
            return Err(Taken);
        }
        let wire = json!({
            "type": "function",
            "function": {
                "name": spec.name,
                "description": template::fill(&spec.description, vars).trim(),
                "parameters": spec.parameters,
            }
        });
        self.entries.push(Entry { tool, wire });
        Ok(())
    }

    /// The tools of this registry that `names` names, in this registry's order, so their wire
    /// list keeps it.
    pub fn subset(&self, names: &[&str]) -> Self {
        let entries = self
            .entries
            .iter()
            .filter(|e| names.contains(&e.tool.spec().name.as_str()))
            .cloned()
            .collect();
        Self { entries }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn AnyTool>> {
        self.entries
            .iter()
            .map(|e| &e.tool)
            .find(|t| t.spec().name == name)
    }

    pub fn each_name(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.tool.spec().name.as_str())
    }

    pub fn names(&self) -> String {
        self.each_name().collect::<Vec<_>>().join(", ")
    }

    /// The `tools` array of a chat request.
    pub fn wire(&self) -> Vec<Value> {
        self.entries.iter().map(|e| e.wire.clone()).collect()
    }

    /// `wire`, leaving out the tools named in `except`.
    pub fn wire_except(&self, except: &[&str]) -> Vec<Value> {
        self.entries
            .iter()
            .filter(|e| !except.contains(&e.tool.spec().name.as_str()))
            .map(|e| e.wire.clone())
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
        for tool in registry.entries.iter().map(|e| &e.tool) {
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

    /// A tool from outside, named and worded by `name` and `description`.
    struct Outside(Spec);

    impl Tool for Outside {
        type Args = Value;
        fn spec(&self) -> &Spec {
            &self.0
        }
        fn check(&self, _: &Value, _: &ToolCx<'_>) -> Result<Check, Refusal> {
            Ok(Check::new(Vec::new()))
        }
        fn run<'a>(&'a self, _: Value, _: &'a ToolCx<'a>) -> BoxFuture<'a, Result<Output, String>> {
            Box::pin(async { Ok(Output::ok("")) })
        }
    }

    fn outside(name: &str, description: &str) -> Arc<dyn AnyTool> {
        Arc::new(Outside(Spec::load(
            name,
            description,
            r#"{"type":"object"}"#,
        )))
    }

    #[test]
    fn a_tool_added_later_keeps_its_words_and_a_taken_name_is_refused() {
        let mut registry = Registry::builtin(None);
        let before = registry.wire();
        registry
            .add(outside("server__echo", "Echoes {max_lines} lines."), &[])
            .unwrap();
        let wire = registry.wire();
        assert_eq!(wire[..before.len()], before[..]);
        let added = &wire[before.len()]["function"];
        assert_eq!(added["description"], "Echoes {max_lines} lines.");
        assert!(registry.add(outside("read", "another read"), &[]).is_err());
        assert_eq!(registry.wire().len(), before.len() + 1);
    }
}
