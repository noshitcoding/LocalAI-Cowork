//! Bounded native control flow. No IO, credentials, expressions or model calls.
use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowControlStep {
    pub name: String,
    pub dependencies: Vec<String>,
    pub operation: WorkflowControlOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowGate {
    pub step_id: String,
    pub when: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputRef {
    pub step_id: String,
    pub pointer: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowControlOperation {
    Review {
        form: WorkflowReviewForm,
        rights: WorkflowRights,
    },
    Wait {
        seconds_input: String,
    },
    Condition {
        predicate: WorkflowPredicate,
    },
    Compose,
    Select {
        candidates: Vec<WorkflowOutputRef>,
    },
    Collect {
        candidates: Vec<WorkflowOutputRef>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "operator", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowPredicate {
    Equal { input: String, value: Box<Value> },
    NotEqual { input: String, value: Box<Value> },
    Greater { input: String, value: Box<Value> },
    Less { input: String, value: Box<Value> },
    Contains { input: String, value: String },
    IsTrue { input: String },
    IsNull { input: String },
    All { predicates: Vec<WorkflowPredicate> },
    Any { predicates: Vec<WorkflowPredicate> },
    Not { predicate: Box<WorkflowPredicate> },
}
fn invalid_control() -> Error {
    Error::Validation("workflow_control_invalid".into())
}
// JSON has one number type. Integer/floating encodings such as 25 and 25.0
// must compare identically, including inside typed objects and lists. Values
// have already passed the finite-number, shape and nesting bounds.
fn equal_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| equal_value(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| equal_value(value, other))
                })
        }
        _ => left == right,
    }
}
impl WorkflowPredicate {
    fn validate(
        &self,
        inputs: &BTreeMap<String, WorkflowValueBinding>,
        depth: usize,
        count: &mut usize,
    ) -> Result<()> {
        *count += 1;
        if depth > 4 || *count > 32 {
            return Err(invalid_control());
        }
        let field = |key: &str| {
            inputs
                .get(key)
                .map(|b| &b.value_type)
                .ok_or_else(invalid_control)
        };
        match self {
            Self::Equal { input, value } | Self::NotEqual { input, value } => {
                field(input)?.validate_value(value)
            }
            Self::Greater { input, value } | Self::Less { input, value } => {
                let ty = field(input)?;
                if !matches!(ty, WorkflowDataType::Number | WorkflowDataType::Integer) {
                    return Err(invalid_control());
                }
                ty.validate_value(value)
            }
            Self::Contains { input, value } => {
                if field(input)? != &WorkflowDataType::Text {
                    return Err(invalid_control());
                }
                WorkflowDataType::Text.validate_value(&Value::String(value.clone()))
            }
            Self::IsTrue { input } => {
                if field(input)? == &WorkflowDataType::Boolean {
                    Ok(())
                } else {
                    Err(invalid_control())
                }
            }
            Self::IsNull { input } => {
                if matches!(
                    field(input)?,
                    WorkflowDataType::Nullable { .. } | WorkflowDataType::Null
                ) {
                    Ok(())
                } else {
                    Err(invalid_control())
                }
            }
            Self::All { predicates } | Self::Any { predicates } => {
                if predicates.is_empty() || predicates.len() > 16 {
                    return Err(invalid_control());
                }
                for predicate in predicates {
                    predicate.validate(inputs, depth + 1, count)?;
                }
                Ok(())
            }
            Self::Not { predicate } => predicate.validate(inputs, depth + 1, count),
        }
    }
    fn evaluate(&self, data: &Value) -> Result<bool> {
        let get = |key: &str| data.get(key).ok_or_else(invalid_control);
        Ok(match self {
            Self::Equal { input, value } => equal_value(get(input)?, value),
            Self::NotEqual { input, value } => !equal_value(get(input)?, value),
            Self::Greater { input, value } => {
                get(input)?.as_f64().ok_or_else(invalid_control)?
                    > value.as_f64().ok_or_else(invalid_control)?
            }
            Self::Less { input, value } => {
                get(input)?.as_f64().ok_or_else(invalid_control)?
                    < value.as_f64().ok_or_else(invalid_control)?
            }
            Self::Contains { input, value } => get(input)?
                .as_str()
                .ok_or_else(invalid_control)?
                .contains(value),
            Self::IsTrue { input } => get(input)?.as_bool().ok_or_else(invalid_control)?,
            Self::IsNull { input } => get(input)?.is_null(),
            Self::All { predicates } => {
                let mut result = true;
                for p in predicates {
                    result &= p.evaluate(data)?;
                }
                result
            }
            Self::Any { predicates } => {
                let mut result = false;
                for p in predicates {
                    result |= p.evaluate(data)?;
                }
                result
            }
            Self::Not { predicate } => !predicate.evaluate(data)?,
        })
    }
}
pub(super) fn add_nodes(
    definition: &WorkflowDefinition,
    scope: Option<&WorkflowScope>,
    nodes: &mut BTreeMap<String, WorkflowNodePlan>,
) -> Result<()> {
    for (id, step) in &definition.controls {
        identifier(id)?;
        if id.starts_with("delegated-") {
            return Err(invalid_control());
        }
        valid_text(&step.name, 200)?;
        let dependencies: BTreeSet<_> = step.dependencies.iter().cloned().collect();
        if dependencies.len() != step.dependencies.len() || nodes.contains_key(id) {
            return Err(invalid_control());
        }
        nodes.insert(
            id.clone(),
            WorkflowNodePlan {
                id: id.clone(),
                role_id: None,
                goal: step.name.clone(),
                dependencies,
                rights: if definition.review_form(id).is_some() {
                    let WorkflowControlOperation::Review { rights, .. } = &step.operation else {
                        return Err(Error::Forbidden);
                    };
                    if rights.tools != BTreeSet::from(["request_input".into()])
                        || !rights.folders.is_empty()
                        || rights.current_workspace
                        || !definition.rights.tools.contains("request_input")
                        || scope.is_some_and(|scope| !scope.rights.tools.contains("request_input"))
                    {
                        return Err(Error::Forbidden);
                    }
                    WorkflowRights {
                        tools: BTreeSet::from(["request_input".into()]),
                        ..Default::default()
                    }
                } else {
                    WorkflowRights::default()
                },
                budgets: if definition.review_form(id).is_some() {
                    scope.map_or_else(
                        || definition.budgets.clone(),
                        |scope| minimum_budgets(&definition.budgets, &scope.budgets),
                    )
                } else {
                    definition.budgets.clone()
                },
                depth: 0,
                parent: None,
            },
        );
    }
    Ok(())
}
pub(super) fn ancestors(
    nodes: &BTreeMap<String, WorkflowNodePlan>,
    node: &str,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut pending: Vec<_> = nodes[node].dependencies.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        if found.insert(id.clone()) {
            pending.extend(nodes[&id].dependencies.iter().cloned());
        }
    }
    found
}
pub(super) fn validate(
    definition: &WorkflowDefinition,
    nodes: &BTreeMap<String, WorkflowNodePlan>,
) -> Result<()> {
    if definition.format_version != 3 {
        return if definition.controls.is_empty() && definition.gates.is_empty() {
            Ok(())
        } else {
            Err(invalid_control())
        };
    }
    if !matches!(definition.mode, WorkflowMode::Parallel { .. }) {
        return Err(invalid_control());
    }
    let data = definition.dataflow.as_ref().ok_or_else(invalid_control)?;
    for (id, gate) in &definition.gates {
        if !nodes.contains_key(id)
            || !ancestors(nodes, id).contains(&gate.step_id)
            || !matches!(
                definition.controls.get(&gate.step_id).map(|s| &s.operation),
                Some(WorkflowControlOperation::Condition { .. })
            )
        {
            return Err(invalid_control());
        }
    }
    for (id, step) in &definition.controls {
        let config = data.steps.get(id).ok_or_else(invalid_control)?;
        match &step.operation {
            WorkflowControlOperation::Review { form, .. } => {
                form.validate()?;
                let prior = ancestors(nodes, id);
                if form.source_steps.iter().any(|source| {
                    !prior.contains(source)
                        || definition.documents.get(source).is_none_or(|document| {
                            !matches!(
                                document.operation,
                                WorkflowDocumentOperation::ReadText { .. }
                                    | WorkflowDocumentOperation::Ocr { .. }
                            )
                        })
                }) {
                    return Err(Error::Validation("workflow_review_source_invalid".into()));
                }
                if config.output != form.output_type()
                    || config.inputs.len() != form.fields.len()
                    || form.fields.iter().any(|(name, field)| {
                        config
                            .inputs
                            .get(name)
                            .is_none_or(|binding| binding.value_type != field.value_type)
                    })
                {
                    return Err(invalid_control());
                }
            }
            WorkflowControlOperation::Wait { seconds_input } => {
                if config.inputs.len() != 1
                    || config.inputs.get(seconds_input).map(|v| &v.value_type)
                        != Some(&WorkflowDataType::Integer)
                    || config.output != wait_output_type()
                {
                    return Err(invalid_control());
                }
                if let WorkflowDataSource::Literal { value } = &config.inputs[seconds_input].value
                    && wait_seconds(value)? as u64
                        >= definition.budgets.max_seconds.saturating_sub(1)
                {
                    return Err(Error::BudgetConfiguration);
                }
            }
            WorkflowControlOperation::Condition { predicate } => {
                if config.output != WorkflowDataType::Boolean {
                    return Err(invalid_control());
                }
                predicate.validate(&config.inputs, 0, &mut 0)?;
            }
            WorkflowControlOperation::Compose => {
                let expected = WorkflowDataType::Object {
                    fields: config
                        .inputs
                        .iter()
                        .map(|(key, b)| (key.clone(), b.value_type.clone()))
                        .collect(),
                };
                if config.output != expected {
                    return Err(invalid_control());
                }
            }
            WorkflowControlOperation::Select { candidates }
            | WorkflowControlOperation::Collect { candidates } => {
                if !config.inputs.is_empty()
                    || (candidates.is_empty()
                        && matches!(step.operation, WorkflowControlOperation::Select { .. }))
                    || candidates.len() > 64
                {
                    return Err(invalid_control());
                }
                let output = match &step.operation {
                    WorkflowControlOperation::Collect { .. } => match &config.output {
                        WorkflowDataType::List { item, max_items }
                            if *max_items as usize >= candidates.len() =>
                        {
                            item.as_ref()
                        }
                        _ => return Err(invalid_control()),
                    },
                    _ => &config.output,
                };
                let ancestors = ancestors(nodes, id);
                let mut seen = BTreeSet::new();
                for source in candidates {
                    if !ancestors.contains(&source.step_id)
                        || !seen.insert((&source.step_id, &source.pointer))
                        || data
                            .steps
                            .get(&source.step_id)
                            .ok_or_else(invalid_control)?
                            .output
                            .at_pointer(&source.pointer)?
                            != output
                    {
                        return Err(invalid_control());
                    }
                }
            }
        }
    }
    Ok(())
}
impl WorkflowDefinition {
    /// None means an empty selection; ordinary control results always have a value.
    pub fn evaluate_control(
        &self,
        node: &str,
        input: Option<&Value>,
        completed: &BTreeMap<String, String>,
    ) -> Result<Option<Value>> {
        self.evaluate_control_at(node, input, completed, None)
    }
    pub fn evaluate_control_at(
        &self,
        node: &str,
        input: Option<&Value>,
        completed: &BTreeMap<String, String>,
        at: Option<i64>,
    ) -> Result<Option<Value>> {
        let step = self.controls.get(node).ok_or_else(invalid_control)?;
        let frame = self.resolve_step_data(node, input, completed)?;
        let value = match &step.operation {
            WorkflowControlOperation::Review { .. } => return Err(Error::Forbidden),
            WorkflowControlOperation::Wait { seconds_input } => {
                let seconds = wait_seconds(frame.get(seconds_input).ok_or_else(invalid_control)?)?;
                let wake_at = at
                    .ok_or_else(invalid_control)?
                    .checked_add(seconds)
                    .ok_or_else(invalid_control)?;
                Some(serde_json::json!({"wake_at":wake_at}))
            }
            WorkflowControlOperation::Condition { predicate } => {
                Some(Value::Bool(predicate.evaluate(&frame)?))
            }
            WorkflowControlOperation::Compose => Some(frame),
            WorkflowControlOperation::Select { candidates }
            | WorkflowControlOperation::Collect { candidates } => {
                let mut values = vec![];
                for source in candidates {
                    if let Some(output) = completed.get(&source.step_id) {
                        values.push(
                            self.decode_step_output(&source.step_id, output)?
                                .pointer(&source.pointer)
                                .cloned()
                                .ok_or_else(invalid_control)?,
                        );
                    }
                }
                if matches!(step.operation, WorkflowControlOperation::Collect { .. }) {
                    Some(Value::Array(values))
                } else {
                    if values.len() > 1 {
                        return Err(Error::Validation(
                            "workflow_control_ambiguous_selection".into(),
                        ));
                    }
                    values.pop()
                }
            }
        };
        if let Some(value) = &value {
            self.dataflow.as_ref().ok_or_else(invalid_control)?.steps[node]
                .output
                .validate_value(value)?;
        }
        Ok(value)
    }
}
pub fn wait_output_type() -> WorkflowDataType {
    WorkflowDataType::Object {
        fields: BTreeMap::from([("wake_at".into(), WorkflowDataType::Integer)]),
    }
}
fn wait_seconds(value: &Value) -> Result<i64> {
    let number = value.as_f64().ok_or_else(invalid_control)?;
    if !number.is_finite() || number.fract() != 0.0 || !(1.0..=86400.0).contains(&number) {
        return Err(invalid_control());
    }
    Ok(number as i64)
}
impl WorkflowPlan {
    /// Evaluate only the bounded deterministic prefix before provider probes.
    /// This disposable graph never reaches storage or returns an IO permit.
    /// Its virtual timers are never advanced: no timestamp-dependent output
    /// or unknown model result may be treated as known preflight data.
    pub fn validate_wait_inputs(&self, input: Option<&Value>) -> Result<()> {
        self.definition.validate_data_input(input)?;
        if self.iterations.is_empty()
            && !self
                .definition
                .controls
                .values()
                .any(|step| matches!(step.operation, WorkflowControlOperation::Wait { .. }))
        {
            return Ok(());
        }
        let mut preview = WorkflowProgress::new(self)?;
        for _ in 0..self.nodes.len() {
            let ready = preview.ready_nodes(self)?;
            let mut selected = None;
            for node in ready {
                if preview.requires_native(self, &node)? {
                    selected = Some(node);
                    break;
                }
            }
            let Some(node) = selected else {
                break;
            };
            let claim = preview.claim(self, preview.revision(), &node, 0)?;
            let id = format!("preflight-{}", preview.revision());
            let graph = preview.execution_graph(self)?;
            let wait = matches!(
                graph
                    .definition
                    .controls
                    .get(&node)
                    .map(|step| &step.operation),
                Some(WorkflowControlOperation::Wait { .. })
            );
            let result = if wait {
                preview.complete_timer(self, preview.revision(), &claim, &id, input, 0)
            } else {
                preview.complete_control(self, preview.revision(), &claim, &id, input, 0)
            };
            match result {
                Err(Error::BudgetExceeded) => return Err(Error::BudgetConfiguration),
                Err(error) => return Err(error),
                Ok(()) => {}
            }
            if let WorkflowNodeState::WaitingTimer { wake_at, .. } = preview.nodes()[&node].state {
                let mut limit = self.budgets.max_seconds;
                let mut current = Some(node.as_str());
                while let Some(id) = current {
                    let plan = &preview.nodes()[id].plan;
                    limit = limit.min(plan.budgets.max_seconds);
                    current = plan.parent.as_deref();
                }
                if wake_at < 1 || wake_at as u64 >= limit.saturating_sub(1) {
                    return Err(Error::BudgetConfiguration);
                }
            }
        }
        Ok(())
    }
}
