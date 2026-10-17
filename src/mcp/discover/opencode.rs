//! opencode's shape: `type: "local"` with `command` as one list (the program, then its
//! arguments) and `environment`, or `type: "remote"` with `url` and `headers`.

use serde_json::Value;

use super::common::{strings, texts};
use crate::mcp::config::Reach;

pub fn reach(entry: &Value) -> Option<Reach> {
    if entry["enabled"] == false {
        return None;
    }
    match entry["type"].as_str() {
        Some("local") | None => {
            let command = strings(&entry["command"]);
            let (program, args) = command.split_first()?;
            Some(Reach::Program {
                command: program.clone(),
                args: args.to_vec(),
                env: texts(&entry["environment"]),
            })
        }
        Some("remote") => Some(Reach::Url {
            url: entry["url"].as_str()?.to_owned(),
            headers: texts(&entry["headers"]),
        }),
        Some(_) => None,
    }
}
