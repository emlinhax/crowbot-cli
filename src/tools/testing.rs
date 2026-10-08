//! A throwaway project and the context tools run in, for tests.

use serde_json::{Value, json};

use super::{AnyTool, Check, Output, Refusal, ToolCx};
use crate::app::App;
use crate::live::state::Shared;

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

    /// Checks a call as the agent does: from the model's JSON.
    pub fn check(&self, tool: &dyn AnyTool, args: &Value) -> Result<Check, Refusal> {
        tool.check(args, &self.cx())
    }

    /// Runs a call as the agent does: from the model's JSON.
    pub async fn run(&self, tool: &dyn AnyTool, args: Value) -> Output {
        tool.run(args, &self.cx()).await
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
