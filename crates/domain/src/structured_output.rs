//! Local-only schema validation. External references are never resolved.
use crate::{Error, Result};
use cowork_contracts::StructuredOutput;
use serde_json::{Value, json};
fn invalid() -> Error {
    Error::Validation("invalid_structured_output_schema".into())
}
fn walk(value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    *nodes += 1;
    if depth > 64 || *nodes > 4096 {
        return Err(invalid());
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if ["$ref", "$dynamicRef", "$recursiveRef"].contains(&key.as_str())
                    && child
                        .as_str()
                        .is_none_or(|s| s != "#" && !s.starts_with("#/"))
                {
                    return Err(invalid());
                }
                if key == "$id"
                    || (key == "$schema"
                        && !matches!(
                            child.as_str(),
                            Some("https://json-schema.org/draft/2020-12/schema")
                        ))
                {
                    return Err(invalid());
                }
                walk(child, depth + 1, nodes)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                walk(child, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn validate_schema(output: &StructuredOutput) -> Result<()> {
    if output.name.is_empty()
        || output.name.len() > 64
        || !output
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || serde_json::to_vec(&output.schema)
            .map_err(|_| invalid())?
            .len()
            > 64 * 1024
        || !output.schema.is_object()
        || output.schema["type"] != "object"
    {
        return Err(invalid());
    }
    walk(&output.schema, 0, &mut 0)?;
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(&output.schema)
        .map_err(|_| invalid())?;
    Ok(())
}
pub fn validate_result(output: &StructuredOutput, content: &str) -> Result<Value> {
    validate_schema(output)?;
    if content.len() > 2 * 1024 * 1024 {
        return Err(Error::Validation("structured_output_too_large".into()));
    }
    let value: Value = serde_json::from_str(content)
        .map_err(|_| Error::Validation("structured_output_invalid_json".into()))?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(&output.schema)
        .map_err(|_| invalid())?;
    if !validator.is_valid(&value) {
        return Err(Error::Validation("structured_output_mismatch".into()));
    }
    Ok(value)
}
/// Exact provider framing; unsupported remote schemas are rejected by the
/// selected provider, never retried through a weaker format or fallback model.
pub fn openai_format(output: &StructuredOutput) -> Result<Value> {
    validate_schema(output)?;
    Ok(
        json!({"type":"json_schema","json_schema":{"name":output.name,"strict":true,"schema":output.schema}}),
    )
}
