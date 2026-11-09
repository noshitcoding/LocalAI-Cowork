//! Deterministic, bounded predicates shared by preview and admission.
use crate::{Error, Result, valid_text};
use cowork_contracts::{EventPredicate, SubmitAutomationEvent};
use serde_json::Value;
mod inputs;
pub use inputs::{resolve_workflow_inputs, validate_input_bindings};

pub const EVENT_MAX_BYTES: usize = 32_768;

pub fn validate_event(event: &SubmitAutomationEvent) -> Result<()> {
    valid_text(&event.event_id, 128)?;
    if event.event_id.chars().any(char::is_control) || !event.data.is_object() {
        return Err(Error::Validation("automation_event_requires_object".into()));
    }
    if serde_json::to_vec(&event.data)
        .map_err(|_| invalid())?
        .len()
        > EVENT_MAX_BYTES
    {
        return Err(Error::Validation("automation_event_too_large".into()));
    }
    let mut remaining = 4096;
    bounded_json(&event.data, 0, &mut remaining)
}

fn invalid() -> Error {
    Error::Validation("automation_filter_invalid".into())
}

fn bounded_json(value: &Value, depth: usize, remaining: &mut usize) -> Result<()> {
    if depth > 32 || *remaining == 0 {
        return Err(invalid());
    }
    *remaining -= 1;
    match value {
        Value::Array(values) => {
            for value in values {
                bounded_json(value, depth + 1, remaining)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                bounded_json(value, depth + 1, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_predicates(predicates: &[EventPredicate]) -> Result<()> {
    if predicates.len() > 32 {
        return Err(invalid());
    }
    for predicate in predicates {
        let pointer = match predicate {
            EventPredicate::Exists { pointer } => pointer,
            EventPredicate::Equals { pointer, value } => {
                if !value.is_null()
                    && !value.is_string()
                    && !value.is_number()
                    && !value.is_boolean()
                {
                    return Err(invalid());
                }
                if value.to_string().len() > 4096 {
                    return Err(invalid());
                }
                pointer
            }
            EventPredicate::Contains { pointer, text } => {
                valid_text(text, 4096)?;
                pointer
            }
            EventPredicate::GreaterThan { pointer, value }
            | EventPredicate::LessThan { pointer, value } => {
                if !value.is_finite() {
                    return Err(invalid());
                }
                pointer
            }
        };
        if pointer.len() > 512 || !pointer.starts_with('/') || pointer.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let mut chars = pointer.chars();
        while let Some(c) = chars.next() {
            if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

pub fn matches_event(predicates: &[EventPredicate], data: &Value) -> Result<bool> {
    validate_predicates(predicates)?;
    Ok(predicates.iter().all(|predicate| match predicate {
        EventPredicate::Exists { pointer } => data.pointer(pointer).is_some(),
        EventPredicate::Equals { pointer, value } => {
            data.pointer(pointer).is_some_and(|v| v == value)
        }
        EventPredicate::Contains { pointer, text } => data
            .pointer(pointer)
            .and_then(Value::as_str)
            .is_some_and(|v| v.contains(text)),
        EventPredicate::GreaterThan { pointer, value } => data
            .pointer(pointer)
            .and_then(Value::as_f64)
            .is_some_and(|v| v > *value),
        EventPredicate::LessThan { pointer, value } => data
            .pointer(pointer)
            .and_then(Value::as_f64)
            .is_some_and(|v| v < *value),
    }))
}
