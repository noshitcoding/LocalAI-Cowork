//! Immutable list materialization for the existing parent workflow journal.
//! These values are not run requests or IO authority. Item frames must be
//! attached to that same parent's graph and attempt ledger before dispatch.
use super::*;
use serde_json::{Value, json};

const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowIterationArgument {
    Item {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pointer: String,
    },
    Index,
    Input {
        pointer: String,
    },
    Literal {
        value: Value,
    },
}

/// Private compiled-source preparation. Public definitions infer its input
/// schema from explicit parent bindings instead of accepting a second schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIterationSource {
    pub format_version: u32,
    pub name: String,
    pub input: WorkflowDataType,
    pub collection_input: String,
    pub arguments: BTreeMap<String, WorkflowIterationArgument>,
    pub body: Box<WorkflowDefinition>,
    pub result: WorkflowOutputRef,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub max_parallel: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIterationItem {
    pub id: String,
    pub index: u16,
    pub item_sha256: String,
    pub input_sha256: String,
}

/// Retains input once, rather than copying shared context into every item.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIterationManifest {
    pub format_version: u32,
    pub node_id: String,
    pub source_sha256: String,
    pub input: Box<Value>,
    pub input_sha256: String,
    pub items: Vec<WorkflowIterationItem>,
    pub sha256: String,
}

/// Verified data only, not a dispatch permit. Cannot be deserialized or cloned.
pub struct WorkflowIterationFrames<'a> {
    manifest: &'a WorkflowIterationManifest,
    source: &'a WorkflowIterationSource,
    output: WorkflowDataType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowIterationFrame {
    pub id: String,
    pub index: u16,
    pub input: Box<Value>,
    pub input_sha256: String,
}

fn invalid_iteration<T>() -> Result<T> {
    invalid("workflow_iteration_invalid")
}

impl WorkflowIterationSource {
    pub fn validate(&self) -> Result<()> {
        if self.format_version != 1
            || !(1..=32).contains(&self.max_parallel)
            || matches!(self.body.mode, WorkflowMode::Hierarchical { .. })
        {
            return invalid_iteration();
        }
        if !self.body.iterations.is_empty() || self.body.format_version == 5 {
            return invalid("workflow_iteration_nested_unsupported");
        }
        valid_text(&self.name, 200)?;
        validate_budgets(&self.budgets)?;
        self.rights.validate()?;
        self.input.validate_type()?;
        let (item, _) = self.collection_type()?;
        let executable = self.body.execution_definition()?;
        executable.validate_definition()?;
        let data = self.body.dataflow.as_ref().ok_or(Error::Forbidden)?;
        let WorkflowDataType::Object { fields } = &data.input else {
            return invalid_iteration();
        };
        if self.arguments.len() > 32 || self.arguments.keys().ne(fields.keys()) {
            return invalid_iteration();
        }
        for (name, binding) in &self.arguments {
            let expected = &fields[name];
            let actual = match binding {
                WorkflowIterationArgument::Item { pointer } => item.at_pointer(pointer)?,
                WorkflowIterationArgument::Index => &WorkflowDataType::Integer,
                WorkflowIterationArgument::Input { pointer } => self.input.at_pointer(pointer)?,
                WorkflowIterationArgument::Literal { value } => {
                    expected.validate_value(value)?;
                    continue;
                }
            };
            if actual != expected {
                return Err(Error::Validation("workflow_data_type_mismatch".into()));
            }
        }
        self.output_type()?.validate_type()?;
        if serde_json::to_vec(self)
            .map_err(|_| Error::Forbidden)?
            .len()
            > MAX_MANIFEST_BYTES
        {
            return invalid_iteration();
        }
        Ok(())
    }

    fn collection_type(&self) -> Result<(&WorkflowDataType, u16)> {
        let WorkflowDataType::Object { fields } = &self.input else {
            return invalid_iteration();
        };
        let Some(WorkflowDataType::List { item, max_items }) = fields.get(&self.collection_input)
        else {
            return invalid_iteration();
        };
        Ok((item, *max_items))
    }

    pub fn output_type(&self) -> Result<WorkflowDataType> {
        let executable = self.body.execution_definition()?;
        let data = executable.dataflow.as_ref().ok_or(Error::Forbidden)?;
        let result = data
            .steps
            .get(&self.result.step_id)
            .ok_or(Error::Forbidden)?;
        Ok(WorkflowDataType::List {
            item: Box::new(result.output.at_pointer(&self.result.pointer)?.clone()),
            max_items: self.collection_type()?.1,
        })
    }

    fn frame(&self, input: &Value, index: usize) -> Result<Value> {
        let item = input
            .get(&self.collection_input)
            .and_then(Value::as_array)
            .and_then(|items| items.get(index))
            .ok_or(Error::NotFound)?;
        let fields = self
            .arguments
            .iter()
            .map(|(name, binding)| {
                let value = match binding {
                    WorkflowIterationArgument::Item { pointer } => {
                        item.pointer(pointer).cloned().ok_or_else(|| {
                            Error::Validation("workflow_iteration_item_pointer_missing".into())
                        })?
                    }
                    WorkflowIterationArgument::Index => json!(index),
                    WorkflowIterationArgument::Input { pointer } => {
                        input.pointer(pointer).cloned().ok_or_else(|| {
                            Error::Validation("workflow_iteration_input_pointer_missing".into())
                        })?
                    }
                    WorkflowIterationArgument::Literal { value } => value.clone(),
                };
                Ok((name.clone(), value))
            })
            .collect::<Result<serde_json::Map<String, Value>>>()?;
        let value = Value::Object(fields);
        self.body
            .dataflow
            .as_ref()
            .ok_or(Error::Forbidden)?
            .input
            .validate_value(&value)?;
        Ok(value)
    }
}

impl WorkflowIterationManifest {
    pub fn freeze(source: &WorkflowIterationSource, node_id: &str, input: &Value) -> Result<Self> {
        source.validate()?;
        identifier(node_id)?;
        if matches!(node_id, "__proto__" | "prototype" | "constructor") {
            return invalid_iteration();
        }
        source.input.validate_value(input)?;
        let collection = input
            .get(&source.collection_input)
            .and_then(Value::as_array)
            .ok_or(Error::Forbidden)?;
        let source_sha256 = canonical_hash(source);
        let input_sha256 = canonical_hash(input);
        let mut items = Vec::with_capacity(collection.len());
        for (index, item) in collection.iter().enumerate() {
            let item_sha256 = canonical_hash(item);
            let frame = source.frame(input, index)?;
            let id = format!(
                "iteration-{}",
                &canonical_hash(&json!({
                    "namespace":"cowork-workflow-iteration-item/1", "node":node_id,
                    "source":source_sha256,"input":input_sha256,"index":index,"item":item_sha256,
                }))[..40]
            );
            items.push(WorkflowIterationItem {
                id,
                index: index.try_into().map_err(|_| Error::Forbidden)?,
                item_sha256,
                input_sha256: canonical_hash(&frame),
            });
        }
        let mut manifest = Self {
            format_version: 1,
            node_id: node_id.into(),
            source_sha256,
            input: Box::new(input.clone()),
            input_sha256,
            items,
            sha256: String::new(),
        };
        manifest.sha256 = canonical_hash(&manifest);
        if serde_json::to_vec(&manifest)
            .map_err(|_| Error::Forbidden)?
            .len()
            > MAX_MANIFEST_BYTES
        {
            return invalid_iteration();
        }
        Ok(manifest)
    }

    /// The expected input hash comes from the parent's original capture, not
    /// from this untrusted serialized manifest. A new hash is not new authority.
    pub fn verify<'a>(
        &'a self,
        source: &'a WorkflowIterationSource,
        node_id: &str,
        expected_input_sha256: &str,
    ) -> Result<WorkflowIterationFrames<'a>> {
        if self.format_version != 1
            || self.node_id != node_id
            || self.input_sha256 != expected_input_sha256
            || self.items.len() > 1000
            || self.source_sha256.len() != 64
            || self.sha256.len() != 64
            || self.items.iter().any(|item| {
                item.id.len() != 50 || item.item_sha256.len() != 64 || item.input_sha256.len() != 64
            })
        {
            return Err(Error::Forbidden);
        }
        let expected = Self::freeze(source, node_id, &self.input)?;
        if canonical_hash(self) != canonical_hash(&expected) {
            return Err(Error::Forbidden);
        }
        Ok(WorkflowIterationFrames {
            manifest: self,
            source,
            output: source.output_type()?,
        })
    }
}

impl WorkflowIterationFrames<'_> {
    pub fn len(&self) -> usize {
        self.manifest.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.manifest.items.is_empty()
    }

    pub fn item(&self, index: usize) -> Result<WorkflowIterationFrame> {
        let receipt = self.manifest.items.get(index).ok_or(Error::NotFound)?;
        let input = self.source.frame(&self.manifest.input, index)?;
        if canonical_hash(&input) != receipt.input_sha256 {
            return Err(Error::Forbidden);
        }
        Ok(WorkflowIterationFrame {
            id: receipt.id.clone(),
            index: receipt.index,
            input: Box::new(input),
            input_sha256: receipt.input_sha256.clone(),
        })
    }

    pub fn output_type(&self) -> &WorkflowDataType {
        &self.output
    }

    /// Call only with completed item-return outputs from the parent graph.
    /// Missing, extra or malformed results are never silently dropped.
    pub fn collect(&self, outputs: &BTreeMap<String, String>) -> Result<Value> {
        if outputs.len() != self.len() {
            return invalid_iteration();
        }
        let WorkflowDataType::List { item, .. } = &self.output else {
            return Err(Error::Forbidden);
        };
        let values = self
            .manifest
            .items
            .iter()
            .map(|entry| {
                let output = outputs.get(&entry.id).ok_or(Error::NotFound)?;
                if output.len() > 256 * 1024 {
                    return invalid_iteration();
                }
                let value = serde_json::from_str::<Value>(output)
                    .map_err(|_| Error::Validation("workflow_data_invalid_json".into()))?;
                item.validate_value(&value)?;
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        let value = Value::Array(values);
        self.output.validate_value(&value)?;
        Ok(value)
    }
}
