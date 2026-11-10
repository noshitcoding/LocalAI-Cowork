//! Versioned event-to-input projection. Values cannot select models or rights.
use crate::{
    Error, Result,
    workflows::{WorkflowDataType, WorkflowDefinition},
};
use cowork_contracts::AutomationWorkflowInputs;
use serde_json::{Map, Value};

fn invalid() -> Error {
    Error::Validation("automation_inputs_invalid".into())
}
fn missing() -> Error {
    Error::Validation("automation_inputs_value_missing".into())
}
fn mismatch() -> Error {
    Error::Validation("automation_inputs_type_mismatch".into())
}

pub fn validate_input_bindings(
    mapping: &AutomationWorkflowInputs,
    definition: &WorkflowDefinition,
) -> Result<()> {
    let data = definition
        .dataflow
        .as_ref()
        .ok_or_else(|| Error::Validation("automation_inputs_workflow_required".into()))?;
    data.input.validate_type().map_err(|_| invalid())?;
    if mapping.format_version != 1 || mapping.bindings.len() > 64 {
        return Err(invalid());
    }
    let targets: Vec<_> = mapping.bindings.keys().collect();
    for (index, target) in targets.iter().enumerate() {
        data.input.at_pointer(target).map_err(|_| invalid())?;
        if targets
            .iter()
            .skip(index + 1)
            .any(|other| target.is_empty() || other.starts_with(&format!("{target}/")))
        {
            return Err(invalid());
        }
    }
    for source in mapping.bindings.values() {
        if source.len() > 512
            || source.chars().any(char::is_control)
            || (!source.is_empty() && !source.starts_with('/'))
        {
            return Err(invalid());
        }
        let mut chars = source.chars();
        while let Some(c) = chars.next() {
            if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// Objects may be assembled from declared fields. List positions must already
/// exist in the task's saved list; mapping a whole list is always supported.
fn put(current: &mut Value, kind: &WorkflowDataType, path: &[&str], value: Value) -> Result<()> {
    let Some((key, rest)) = path.split_first() else {
        *current = value;
        return Ok(());
    };
    match kind {
        WorkflowDataType::Object { fields } => {
            let child_type = fields.get(*key).ok_or_else(invalid)?;
            let object = current.as_object_mut().ok_or_else(mismatch)?;
            if rest.is_empty() {
                object.insert((*key).into(), value);
                return Ok(());
            }
            let child = object.entry((*key).to_owned()).or_insert_with(|| {
                if matches!(child_type, WorkflowDataType::Object { .. }) {
                    Value::Object(Map::new())
                } else {
                    Value::Null
                }
            });
            put(child, child_type, rest, value)
        }
        WorkflowDataType::List { item, .. } => {
            let index: usize = key.parse().map_err(|_| invalid())?;
            let child = current
                .as_array_mut()
                .ok_or_else(mismatch)?
                .get_mut(index)
                .ok_or_else(missing)?;
            put(child, item, rest, value)
        }
        _ => Err(invalid()),
    }
}

pub fn resolve_workflow_inputs(
    mapping: &AutomationWorkflowInputs,
    definition: &WorkflowDefinition,
    saved: Option<&Value>,
    event: &Value,
) -> Result<Value> {
    validate_input_bindings(mapping, definition)?;
    if let Some(saved) = saved {
        crate::workflows::validate_data_value_budget(saved)?;
    }
    let input_type = &definition.dataflow.as_ref().ok_or_else(invalid)?.input;
    let mut result = saved.cloned().unwrap_or_else(|| Value::Object(Map::new()));
    for (target, source) in &mapping.bindings {
        let value = event.pointer(source).ok_or_else(missing)?;
        input_type
            .at_pointer(target)?
            .validate_value(value)
            .map_err(|_| mismatch())?;
        let tokens: Vec<_> = if target.is_empty() {
            vec![]
        } else {
            target[1..].split('/').collect()
        };
        put(&mut result, input_type, &tokens, value.clone())?;
    }
    definition
        .validate_data_input(Some(&result))
        .map_err(|_| mismatch())?;
    Ok(result)
}
