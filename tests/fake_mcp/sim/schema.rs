//! Arguments checked against a tool's own schema, as a real server's validator would before it
//! acts: `type`, `required`, `enum`, `properties`, `additionalProperties: false` and `items`. The
//! first problem found is the answer, in the words such validators use.

use serde_json::Value;

pub fn check(schema: &Value, value: &Value) -> Result<(), String> {
    at(schema, value, "arguments")
}

fn at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(kind) = schema["type"].as_str()
        && !is(kind, value)
    {
        return Err(format!(
            "{path}: expected {kind}, got {} {}",
            kind_of(value),
            value
        ));
    }
    if let Some(allowed) = schema["enum"].as_array()
        && !allowed.contains(value)
    {
        let names: Vec<String> = allowed.iter().map(Value::to_string).collect();
        return Err(format!(
            "{path}: {value} is not one of {}",
            names.join(", ")
        ));
    }
    if let Value::Object(fields) = value {
        let properties = schema["properties"].as_object();
        for required in schema["required"].as_array().into_iter().flatten() {
            let name = required.as_str().unwrap_or_default();
            if !fields.contains_key(name) {
                return Err(format!("{path}: missing required field `{name}`"));
            }
        }
        for (name, field) in fields {
            match properties.and_then(|p| p.get(name)) {
                Some(inner) => at(inner, field, &format!("{path}.{name}"))?,
                None if schema["additionalProperties"] == false => {
                    let known: Vec<&String> =
                        properties.into_iter().flatten().map(|(k, _)| k).collect();
                    let known: Vec<&str> = known.iter().map(|k| k.as_str()).collect();
                    return Err(format!(
                        "{path}: unknown field `{name}` (fields: {})",
                        known.join(", ")
                    ));
                }
                None => {}
            }
        }
    }
    if let (Value::Array(items), Some(item)) = (value, schema.get("items")) {
        for (i, each) in items.iter().enumerate() {
            at(item, each, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn is(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}
