//! The shape most apps write a server in: `command`, `args` and `env` for a program, or `url`
//! (`serverUrl`, `httpUrl`) and `headers` for one over HTTP. `disabled: true` or
//! `enabled: false` turns it off.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::mcp::config::Reach;

/// The transports crowbot speaks, as apps name them in `type`; none named is one of these too.
const SPOKEN: &[&str] = &["stdio", "http", "streamable-http", "streamablehttp"];

pub fn reach(entry: &Value) -> Option<Reach> {
    if entry["disabled"] == true || entry["enabled"] == false {
        return None;
    }
    if let Some(kind) = entry["type"].as_str()
        && !SPOKEN.contains(&kind.to_ascii_lowercase().as_str())
    {
        return None;
    }
    if let Some(command) = entry["command"].as_str() {
        return Some(Reach::Program {
            command: command.to_owned(),
            args: strings(&entry["args"]),
            env: texts(&entry["env"]),
        });
    }
    let url = ["url", "serverUrl", "httpUrl"]
        .iter()
        .find_map(|key| entry[*key].as_str())?;
    Some(Reach::Url {
        url: url.to_owned(),
        headers: texts(&entry["headers"]),
    })
}

/// A list of strings, its other items dropped.
pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// An object's string values, by key; a number or a flag as written.
pub fn texts(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(_) | Value::Bool(_) => v.to_string(),
                _ => return None,
            };
            Some((k.clone(), text))
        })
        .collect()
}
