//! Bounded typed values and explicit bindings. Data never grants tool authority.
use super::*;
use serde_json::{Map, Value, json};

const MAX_VALUE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDataType {
    Text,
    Number,
    Integer,
    Boolean,
    Null,
    List {
        item: Box<WorkflowDataType>,
        max_items: u16,
    },
    Object {
        fields: BTreeMap<String, WorkflowDataType>,
    },
    Nullable {
        value: Box<WorkflowDataType>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDataSource {
    Literal { value: Value },
    Input { pointer: String },
    Step { step_id: String, pointer: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowValueBinding {
    pub value_type: WorkflowDataType,
    pub value: WorkflowDataSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepData {
    #[serde(default, skip_serializing_if = "WorkflowDataContext::is_inherit")]
    pub context_policy: WorkflowDataContext,
    pub inputs: BTreeMap<String, WorkflowValueBinding>,
    pub output: WorkflowDataType,
}

/// Frozen per-model-step input boundary. Legacy definitions retain their context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDataContext {
    #[default]
    Inherit,
    BoundInputsOnly,
}
impl WorkflowDataContext {
    fn is_inherit(&self) -> bool {
        *self == Self::Inherit
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDataflow {
    pub input: WorkflowDataType,
    pub steps: BTreeMap<String, WorkflowStepData>,
}

fn invalid_data() -> Error {
    Error::Validation("workflow_data_invalid".into())
}
fn valid_key(key: &str) -> Result<()> {
    identifier(key)?;
    if matches!(key, "__proto__" | "prototype" | "constructor") {
        return Err(invalid_data());
    }
    Ok(())
}
fn value_budget(value: &Value) -> Result<()> {
    fn walk(value: &Value, depth: usize, count: &mut usize) -> Result<()> {
        *count += 1;
        if depth > 16 || *count > 4096 {
            return Err(invalid_data());
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    walk(value, depth + 1, count)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    walk(value, depth + 1, count)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    if serde_json::to_vec(value).map_err(|_| invalid_data())?.len() > MAX_VALUE_BYTES {
        return Err(invalid_data());
    }
    walk(value, 0, &mut 0)
}
/// Draft inputs may be temporarily incomplete, but cannot exceed value budgets.
pub fn validate_data_value_budget(value: &Value) -> Result<()> {
    value_budget(value)
}

impl WorkflowDataType {
    pub fn validate_type(&self) -> Result<()> {
        fn walk(kind: &WorkflowDataType, depth: usize, count: &mut usize) -> Result<()> {
            *count += 1;
            if depth > 8 || *count > 256 {
                return Err(invalid_data());
            }
            match kind {
                WorkflowDataType::List { item, max_items } => {
                    if !(1..=1000).contains(max_items) {
                        return Err(invalid_data());
                    }
                    walk(item, depth + 1, count)?;
                }
                WorkflowDataType::Object { fields } => {
                    if fields.len() > 64 {
                        return Err(invalid_data());
                    }
                    for (key, kind) in fields {
                        valid_key(key)?;
                        walk(kind, depth + 1, count)?;
                    }
                }
                WorkflowDataType::Nullable { value } => {
                    if matches!(
                        **value,
                        WorkflowDataType::Nullable { .. } | WorkflowDataType::Null
                    ) {
                        return Err(invalid_data());
                    }
                    walk(value, depth + 1, count)?;
                }
                _ => {}
            }
            Ok(())
        }
        walk(self, 0, &mut 0)
    }
    pub fn validate_value(&self, value: &Value) -> Result<()> {
        self.validate_type()?;
        value_budget(value)?;
        fn matches(kind: &WorkflowDataType, value: &Value) -> bool {
            match kind {
                WorkflowDataType::Text => value
                    .as_str()
                    .is_some_and(|v| v.chars().count() <= 16384 && !v.contains('\0')),
                WorkflowDataType::Number => value.is_number(),
                WorkflowDataType::Integer => value
                    .as_f64()
                    .is_some_and(|v| v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0),
                WorkflowDataType::Boolean => value.is_boolean(),
                WorkflowDataType::Null => value.is_null(),
                WorkflowDataType::List { item, max_items } => {
                    value.as_array().is_some_and(|values| {
                        values.len() <= *max_items as usize
                            && values.iter().all(|v| matches(item, v))
                    })
                }
                WorkflowDataType::Object { fields } => value.as_object().is_some_and(|values| {
                    values.len() == fields.len()
                        && fields
                            .iter()
                            .all(|(key, kind)| values.get(key).is_some_and(|v| matches(kind, v)))
                }),
                WorkflowDataType::Nullable { value: kind } => {
                    value.is_null() || matches(kind, value)
                }
            }
        }
        if matches(self, value) {
            Ok(())
        } else {
            Err(Error::Validation("workflow_data_type_mismatch".into()))
        }
    }
    pub fn schema(&self) -> Value {
        match self {
            Self::Text => json!({"type":"string","maxLength":16384,"pattern":"^[^\\u0000]*$"}),
            Self::Number => json!({"type":"number"}),
            Self::Integer => {
                json!({"type":"integer","minimum":-9_007_199_254_740_991i64,"maximum":9_007_199_254_740_991i64})
            }
            Self::Boolean => json!({"type":"boolean"}),
            Self::Null => json!({"type":"null"}),
            Self::List { item, max_items } => {
                json!({"type":"array","items":item.schema(),"maxItems":max_items})
            }
            Self::Object { fields } => {
                json!({"type":"object","properties":fields.iter().map(|(key, kind)|(key.clone(),kind.schema())).collect::<Map<_,_>>(),"required":fields.keys().collect::<Vec<_>>(),"additionalProperties":false})
            }
            Self::Nullable { value } => json!({"anyOf":[{"type":"null"},value.schema()]}),
        }
    }
    pub fn at_pointer(&self, pointer: &str) -> Result<&Self> {
        if pointer.is_empty() {
            return Ok(self);
        }
        if pointer.len() > 1024 || !pointer.starts_with('/') {
            return Err(invalid_data());
        }
        let mut current = self;
        for token in pointer[1..].split('/') {
            // Supported identifiers contain neither slash nor tilde; no escape
            // can designate a supported key. Array indices use canonical form.
            if token.contains('~') || token.is_empty() {
                return Err(invalid_data());
            }
            current = match current {
                Self::Object { fields } => fields.get(token).ok_or_else(invalid_data)?,
                Self::List { item, max_items } => {
                    if !token.bytes().all(|value| value.is_ascii_digit()) {
                        return Err(invalid_data());
                    }
                    if token.len() > 1 && token.starts_with('0') {
                        return Err(invalid_data());
                    }
                    let index: usize = token.parse().map_err(|_| invalid_data())?;
                    if index >= *max_items as usize {
                        return Err(invalid_data());
                    }
                    item
                }
                _ => return Err(invalid_data()),
            };
        }
        Ok(current)
    }
}

pub(super) fn validate_definition(
    definition: &WorkflowDefinition,
    nodes: &BTreeMap<String, WorkflowNodePlan>,
) -> Result<()> {
    let Some(data) = &definition.dataflow else {
        return if definition.format_version == 1 {
            Ok(())
        } else {
            Err(invalid_data())
        };
    };
    if !matches!(definition.format_version, 2 | 3)
        || data.steps.len() != nodes.len()
        || !matches!(data.input, WorkflowDataType::Object { .. })
        || definition
            .roles
            .iter()
            .any(|role| !role.delegate_to.is_empty() || role.output.is_some())
    {
        return Err(invalid_data());
    }
    data.input.validate_type()?;
    for id in nodes.keys() {
        let step = definition.steps.iter().find(|s| &s.id == id);
        let native = definition.controls.contains_key(id);
        let web = step.is_some_and(|s| s.execution.is_some());
        let config = data.steps.get(id).ok_or_else(invalid_data)?;
        if config.context_policy == WorkflowDataContext::BoundInputsOnly && (step.is_none() || web)
        {
            return Err(invalid_data());
        }
        config.output.validate_type()?;
        if config.inputs.len() > 32
            || (web && !config.inputs.is_empty())
            || if web {
                config.output != WorkflowDataType::Text
            } else {
                !native && !matches!(config.output, WorkflowDataType::Object { .. })
            }
        {
            return Err(invalid_data());
        }
        let mut ancestors = BTreeSet::new();
        let mut pending: Vec<_> = nodes[id].dependencies.iter().cloned().collect();
        while let Some(id) = pending.pop() {
            if ancestors.insert(id.clone()) {
                pending.extend(nodes[&id].dependencies.iter().cloned());
            }
        }
        for (key, binding) in &config.inputs {
            valid_key(key)?;
            binding.value_type.validate_type()?;
            let source_type = match &binding.value {
                WorkflowDataSource::Literal { value } => {
                    binding.value_type.validate_value(value)?;
                    continue;
                }
                WorkflowDataSource::Input { pointer } => data.input.at_pointer(pointer)?,
                WorkflowDataSource::Step { step_id, pointer } => {
                    if !ancestors.contains(step_id) {
                        return Err(Error::Validation("workflow_data_dependency_missing".into()));
                    }
                    data.steps
                        .get(step_id)
                        .ok_or_else(invalid_data)?
                        .output
                        .at_pointer(pointer)?
                }
            };
            if source_type != &binding.value_type {
                return Err(Error::Validation("workflow_data_type_mismatch".into()));
            }
        }
    }
    Ok(())
}

impl WorkflowDefinition {
    pub fn validate_data_input(&self, input: Option<&Value>) -> Result<()> {
        match (&self.dataflow, input) {
            (None, None) => Ok(()),
            (Some(data), Some(input)) => data.input.validate_value(input),
            _ => Err(Error::Validation("workflow_data_input_required".into())),
        }
    }
    pub fn decode_step_output(&self, node: &str, output: &str) -> Result<Value> {
        let config = self
            .dataflow
            .as_ref()
            .and_then(|data| data.steps.get(node))
            .ok_or_else(invalid_data)?;
        let step = self.steps.iter().find(|step| step.id == node);
        if step.is_none() && !self.controls.contains_key(node) && !self.documents.contains_key(node)
        {
            return Err(invalid_data());
        }
        if output.len() > MAX_VALUE_BYTES {
            return Err(invalid_data());
        }
        let value = if step.is_some_and(|step| step.execution.is_some()) {
            Value::String(output.into())
        } else {
            serde_json::from_str(output)
                .map_err(|_| Error::Validation("workflow_data_invalid_json".into()))?
        };
        config.output.validate_value(&value)?;
        if let Some(form) = self.review_form(node) {
            form.validate_answer(&value)?;
        }
        Ok(value)
    }
    pub fn step_output_format(&self, node: &str) -> Result<Option<StructuredOutput>> {
        let Some(data) = &self.dataflow else {
            return Ok(None);
        };
        let step = self.steps.iter().find(|step| step.id == node);
        if step.is_none() && !self.controls.contains_key(node) && !self.documents.contains_key(node)
        {
            return Err(invalid_data());
        }
        if self.controls.contains_key(node)
            || self.documents.contains_key(node)
            || step.is_some_and(|step| step.execution.is_some())
        {
            return Ok(None);
        }
        let output = StructuredOutput {
            name: "workflow_result".into(),
            schema: data
                .steps
                .get(node)
                .ok_or_else(invalid_data)?
                .output
                .schema(),
        };
        crate::structured_output::validate_schema(&output)?;
        Ok(Some(output))
    }
    pub fn resolve_step_data(
        &self,
        node: &str,
        input: Option<&Value>,
        completed: &BTreeMap<String, String>,
    ) -> Result<Value> {
        self.validate_data_input(input)?;
        let data = self.dataflow.as_ref().ok_or_else(invalid_data)?;
        let config = data.steps.get(node).ok_or_else(invalid_data)?;
        let mut result = Map::new();
        for (key, binding) in &config.inputs {
            let value = match &binding.value {
                WorkflowDataSource::Literal { value } => value.clone(),
                WorkflowDataSource::Input { pointer } => input
                    .and_then(|value| value.pointer(pointer))
                    .cloned()
                    .ok_or_else(|| Error::Validation("workflow_data_value_missing".into()))?,
                WorkflowDataSource::Step { step_id, pointer } => self
                    .decode_step_output(
                        step_id,
                        completed.get(step_id).ok_or_else(|| {
                            Error::Validation("workflow_data_value_missing".into())
                        })?,
                    )?
                    .pointer(pointer)
                    .cloned()
                    .ok_or_else(|| Error::Validation("workflow_data_value_missing".into()))?,
            };
            binding.value_type.validate_value(&value)?;
            result.insert(key.clone(), value);
        }
        let value = Value::Object(result);
        value_budget(&value)?;
        Ok(value)
    }
}
