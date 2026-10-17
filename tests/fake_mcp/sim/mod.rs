//! A server a file describes (load.rs). Its tools are listed in pages; a call's arguments are
//! checked against the tool's own schema (schema.rs), then its state flag, then a busy first
//! call, before it acts on the data (act.rs). Every call and every change is kept when asked
//! (record.rs).

mod act;
mod load;
mod record;
mod schema;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Call;
use act::{Collections, Fail, Outcome};
use load::Definition;
use record::Record;

pub struct Server {
    definition: Definition,
    /// Each tool's schema, parsed once.
    schemas: Vec<Value>,
    data: Collections,
    flags: BTreeSet<String>,
    /// Tools whose busy first call has happened.
    failed_once: BTreeSet<String>,
    record: Option<Record>,
}

impl Server {
    pub fn load(path: &Path, record: Option<PathBuf>) -> Self {
        let mut definition = load::load(path);
        let data = std::mem::take(&mut definition.collections);
        let schemas = definition
            .tools
            .iter()
            .map(|t| serde_json::from_str(&t.schema).expect("checked at load"))
            .collect();
        let record = record.map(|dir| Record::open(&dir, &definition.name));
        let server = Self {
            definition,
            schemas,
            data,
            flags: BTreeSet::new(),
            failed_once: BTreeSet::new(),
            record,
        };
        if let Some(record) = &server.record {
            record.state(&server.flags, &server.data);
        }
        server
    }

    pub fn instructions(&self) -> Option<String> {
        self.definition.instructions.clone()
    }

    /// One page of `tools/list`, from the cursor the last page gave (`t<N>`).
    pub fn list(&self, cursor: Option<&str>) -> Value {
        let tools: Vec<Value> = self
            .definition
            .tools
            .iter()
            .zip(&self.schemas)
            .map(|(tool, schema)| {
                let mut listed = json!({
                    "name": tool.name, "description": tool.description, "inputSchema": schema,
                });
                if tool.read_only {
                    listed["annotations"] = json!({"readOnlyHint": true});
                }
                listed
            })
            .collect();
        let size = match self.definition.list_page {
            0 => tools.len().max(1),
            n => n,
        };
        let start = cursor
            .and_then(|c| c.strip_prefix('t')?.parse().ok())
            .unwrap_or(0usize)
            .min(tools.len());
        let end = (start + size).min(tools.len());
        let mut page = json!({"tools": tools[start..end]});
        if end < tools.len() {
            page["nextCursor"] = json!(format!("t{end}"));
        }
        page
    }

    pub fn call(&mut self, params: &Value) -> Call {
        let name = params["name"].as_str().unwrap_or_default();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let Some(at) = self.definition.tools.iter().position(|t| t.name == name) else {
            if let Some(record) = &mut self.record {
                record.call(name, &args, Outcome::UnknownTool, None);
            }
            return Call::Unknown;
        };
        let Self {
            definition,
            schemas,
            data,
            flags,
            failed_once,
            record,
        } = self;
        let tool = &definition.tools[at];
        if tool.delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(tool.delay_ms));
        }
        let done = (|| {
            let Value::Object(map) = &args else {
                return Err(Fail::new(
                    Outcome::InvalidArgs,
                    "arguments must be an object",
                ));
            };
            schema::check(&schemas[at], &args).map_err(|e| Fail::new(Outcome::InvalidArgs, e))?;
            if let Some(flag) = &tool.requires
                && !flags.contains(flag)
            {
                let missing = definition
                    .flags
                    .get(flag)
                    .map_or("not ready", |f| &f.missing);
                return Err(Fail::new(Outcome::Precondition, missing));
            }
            if let Some(busy) = &tool.fail_first
                && failed_once.insert(tool.name.clone())
            {
                return Err(Fail::new(Outcome::Transient, busy.clone()));
            }
            act::act(tool, map, data)
        })();
        let (text, error) = match done {
            Ok((text, changed)) => {
                let set = tool.sets.as_ref().is_some_and(|f| flags.insert(f.clone()));
                if let Some(record) = record {
                    record.call(name, &args, Outcome::Ok, None);
                    if changed || set {
                        record.state(flags, data);
                    }
                }
                (text, false)
            }
            Err(fail) => {
                if let Some(record) = record {
                    record.call(name, &args, fail.outcome, Some(&fail.text));
                }
                (fail.text, true)
            }
        };
        Call::Answer(json!({"content": [{"type": "text", "text": text}], "isError": error}))
    }
}
