//! Bounded document proposals. These values are data, never permission for IO.
use super::*;
use cowork_contracts::ToolCall;
use serde_json::{Value, json};
mod tables;
pub use tables::{WorkflowTableColumn, WorkflowTableColumnKind};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDocumentFolder {
    CurrentWorkspace,
    SelectedFolder { folder_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDocumentFormat {
    Docx,
    Pdf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDocumentOperation {
    ReadText {
        path_input: String,
        sha256_input: String,
        offset: u32,
    },
    Preview {
        path_input: String,
        sha256_input: String,
        pages: Vec<u32>,
    },
    Ocr {
        path_input: String,
        sha256_input: String,
        languages: Vec<String>,
        pages: Vec<u32>,
        dpi: u32,
        rotation: u16,
        offset: u32,
        max_words: u16,
    },
    Create {
        format: WorkflowDocumentFormat,
        path_input: String,
        content_input: String,
    },
    AppendTableRows {
        path_input: String,
        sha256_input: String,
        output_path_input: String,
        rows_input: String,
        sheet: String,
        table: String,
        columns: Vec<WorkflowTableColumn>,
        key_fields: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDocumentStep {
    pub name: String,
    pub dependencies: Vec<String>,
    pub folder: WorkflowDocumentFolder,
    pub operation: WorkflowDocumentOperation,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
}

fn invalid_document() -> Error {
    Error::Validation("workflow_document_step_invalid".into())
}
fn object(fields: &[(&str, WorkflowDataType)]) -> WorkflowDataType {
    WorkflowDataType::Object {
        fields: fields
            .iter()
            .map(|(key, ty)| ((*key).into(), ty.clone()))
            .collect(),
    }
}
fn source_type() -> WorkflowDataType {
    object(&[
        ("folder_id", WorkflowDataType::Text),
        ("path", WorkflowDataType::Text),
        ("sha256", WorkflowDataType::Text),
    ])
}

fn ocr_output_type() -> WorkflowDataType {
    use WorkflowDataType::{Boolean, Integer, Number, Text};
    let nullable = |value| WorkflowDataType::Nullable {
        value: Box::new(value),
    };
    let list = |item, max_items| WorkflowDataType::List {
        item: Box::new(item),
        max_items,
    };
    object(&[
        ("source", source_type()),
        (
            "engine",
            object(&[
                ("name", Text),
                ("version", Text),
                ("binding_version", Text),
                ("languages", list(Text, 2)),
                (
                    "models",
                    list(object(&[("language", Text), ("sha256", Text)]), 2),
                ),
            ]),
        ),
        ("total_pages", Integer),
        (
            "pages",
            list(
                object(&[
                    ("page", Integer),
                    ("width", Integer),
                    ("height", Integer),
                    ("dpi", Integer),
                    ("rotation", Integer),
                    ("raster_sha256", Text),
                    ("preview_sha256", nullable(Text)),
                    ("word_count", Integer),
                ]),
                16,
            ),
        ),
        ("pages_truncated", Boolean),
        ("next_page", nullable(Integer)),
        ("total_words", Integer),
        ("offset", Integer),
        ("next_offset", nullable(Integer)),
        (
            "words",
            list(
                object(&[
                    ("page", Integer),
                    ("index", Integer),
                    ("line", Integer),
                    ("text", Text),
                    ("text_truncated", Boolean),
                    ("x", Integer),
                    ("y", Integer),
                    ("width", Integer),
                    ("height", Integer),
                    ("score", Number),
                ]),
                100,
            ),
        ),
        ("text", Text),
        ("truncated", Boolean),
    ])
}

impl WorkflowDocumentOperation {
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::Create { .. } | Self::AppendTableRows { .. } => "document_write",
            Self::ReadText { .. } | Self::Preview { .. } | Self::Ocr { .. } => "document_read",
        }
    }
    pub fn input_names(&self) -> Vec<&str> {
        match self {
            Self::ReadText {
                path_input,
                sha256_input,
                ..
            }
            | Self::Preview {
                path_input,
                sha256_input,
                ..
            }
            | Self::Ocr {
                path_input,
                sha256_input,
                ..
            } => vec![path_input, sha256_input],
            Self::Create {
                path_input,
                content_input,
                ..
            } => vec![path_input, content_input],
            Self::AppendTableRows {
                path_input,
                sha256_input,
                output_path_input,
                rows_input,
                ..
            } => vec![path_input, sha256_input, output_path_input, rows_input],
        }
    }
    pub fn input_types(&self) -> BTreeMap<String, WorkflowDataType> {
        let mut fields: BTreeMap<_, _> = self
            .input_names()
            .iter()
            .map(|name| ((*name).to_owned(), WorkflowDataType::Text))
            .collect();
        if let Self::AppendTableRows {
            rows_input,
            columns,
            ..
        } = self
        {
            fields.insert(rows_input.clone(), tables::rows_type(columns));
        }
        fields
    }
    pub fn output_type(&self) -> WorkflowDataType {
        let mut output = match self {
            Self::AppendTableRows { .. } => tables::output_type(),
            Self::Ocr { .. } => ocr_output_type(),
            Self::ReadText { .. } => object(&[
                ("source", source_type()),
                ("text", WorkflowDataType::Text),
                ("truncated", WorkflowDataType::Boolean),
                ("offset", WorkflowDataType::Integer),
                (
                    "next_offset",
                    WorkflowDataType::Nullable {
                        value: Box::new(WorkflowDataType::Integer),
                    },
                ),
            ]),
            Self::Preview { .. } => object(&[
                ("source", source_type()),
                ("truncated", WorkflowDataType::Boolean),
                ("total_pages", WorkflowDataType::Integer),
                (
                    "images",
                    WorkflowDataType::List {
                        item: Box::new(object(&[
                            ("sha256", WorkflowDataType::Text),
                            ("mime_type", WorkflowDataType::Text),
                            ("page", WorkflowDataType::Integer),
                            ("width", WorkflowDataType::Integer),
                            ("height", WorkflowDataType::Integer),
                        ])),
                        max_items: 16,
                    },
                ),
            ]),
            Self::Create { .. } => object(&[(
                "file",
                object(&[
                    ("folder_id", WorkflowDataType::Text),
                    ("path", WorkflowDataType::Text),
                    ("sha256", WorkflowDataType::Text),
                    ("mime_type", WorkflowDataType::Text),
                    ("bytes", WorkflowDataType::Integer),
                ]),
            )]),
        };
        let WorkflowDataType::Object { fields } = &mut output else {
            unreachable!("native document results are objects");
        };
        fields.insert(
            "diagnostics".into(),
            WorkflowDataType::List {
                item: Box::new(object(&[
                    ("severity", WorkflowDataType::Text),
                    ("message", WorkflowDataType::Text),
                    (
                        "file",
                        WorkflowDataType::Nullable {
                            value: Box::new(WorkflowDataType::Text),
                        },
                    ),
                    (
                        "line",
                        WorkflowDataType::Nullable {
                            value: Box::new(WorkflowDataType::Integer),
                        },
                    ),
                ])),
                max_items: 64,
            },
        );
        fields.insert("diagnostics_truncated".into(), WorkflowDataType::Boolean);
        output
    }
    fn validate(&self) -> Result<()> {
        let names = self.input_names();
        for name in &names {
            identifier(name)?;
        }
        if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
            return Err(invalid_document());
        }
        match self {
            Self::AppendTableRows {
                sheet,
                table,
                columns,
                key_fields,
                ..
            } => tables::validate_schema(sheet, table, columns, key_fields),
            Self::Ocr {
                languages,
                pages,
                dpi,
                rotation,
                offset,
                max_words,
                ..
            } if languages.is_empty()
                || languages.len() > 2
                || languages
                    .iter()
                    .any(|value| !matches!(value.as_str(), "deu" | "eng"))
                || languages.iter().collect::<BTreeSet<_>>().len() != languages.len()
                || pages.len() > 16
                || pages.iter().any(|page| *page == 0 || *page > 10_000)
                || pages.iter().collect::<BTreeSet<_>>().len() != pages.len()
                || !(100..=400).contains(dpi)
                || !matches!(rotation, 0 | 90 | 180 | 270)
                || *offset > 1_000_000_000
                || *max_words == 0
                || *max_words > 100 =>
            {
                Err(invalid_document())
            }
            Self::ReadText { offset, .. } if *offset > 1_000_000_000 => Err(invalid_document()),
            Self::Preview { pages, .. }
                if pages.len() > 16
                    || pages.iter().any(|page| *page == 0 || *page > 10_000)
                    || pages.iter().collect::<BTreeSet<_>>().len() != pages.len() =>
            {
                Err(invalid_document())
            }
            _ => Ok(()),
        }
    }
}

impl WorkflowDocumentStep {
    /// A deterministic proposal still needs original plan, lease and tool admission.
    pub fn tool_call(&self, frame: &Value) -> Result<ToolCall> {
        self.operation.validate()?;
        let WorkflowDocumentFolder::SelectedFolder { folder_id } = &self.folder else {
            return Err(Error::Forbidden);
        };
        let fields = frame.as_object().ok_or_else(invalid_document)?;
        let names = self.operation.input_names();
        if fields.len() != names.len() {
            return Err(invalid_document());
        }
        for (name, kind) in self.operation.input_types() {
            kind.validate_value(fields.get(&name).ok_or_else(invalid_document)?)?;
        }
        let value = |name: &str| {
            fields
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(invalid_document)
        };
        let path = value(names[0])?;
        let arguments = match &self.operation {
            WorkflowDocumentOperation::AppendTableRows {
                sha256_input,
                output_path_input,
                rows_input,
                sheet,
                table,
                columns,
                key_fields,
                ..
            } => {
                let sha = value(sha256_input)?;
                let output = value(output_path_input)?;
                if !crate::workspace_transfer::valid_sha256(sha)
                    || !path.to_ascii_lowercase().ends_with(".xlsx")
                    || !output.to_ascii_lowercase().ends_with(".xlsx")
                    || output == path
                {
                    return Err(invalid_document());
                }
                let patch = tables::patch(sheet, table, columns, key_fields, &fields[rows_input])?;
                json!({"operation":"edit","inputs":[{"folder_id":folder_id,"path":path,"expected_sha256":sha}],
                    "output":{"folder_id":folder_id,"path":output},"spec":{"source_sha256":sha,"patches":[patch]}})
            }
            WorkflowDocumentOperation::Ocr {
                languages,
                pages,
                dpi,
                rotation,
                offset,
                max_words,
                ..
            } => {
                let sha = value(names[1])?;
                if !crate::workspace_transfer::valid_sha256(sha) {
                    return Err(invalid_document());
                }
                let mut args = json!({"operation":"ocr","folder_id":folder_id,"path":path,"expected_sha256":sha,
                    "languages":languages,"dpi":dpi,"rotation":rotation,"offset":offset,"limit":max_words});
                if !pages.is_empty() {
                    args["pages"] = json!(pages);
                }
                args
            }
            WorkflowDocumentOperation::ReadText { offset, .. } => {
                let sha = value(names[1])?;
                if !crate::workspace_transfer::valid_sha256(sha) {
                    return Err(invalid_document());
                }
                json!({"operation":"text","folder_id":folder_id,"path":path,"expected_sha256":sha,"offset":offset})
            }
            WorkflowDocumentOperation::Preview { pages, .. } => {
                let sha = value(names[1])?;
                if !crate::workspace_transfer::valid_sha256(sha) {
                    return Err(invalid_document());
                }
                let mut args = json!({"operation":"preview","folder_id":folder_id,"path":path,"expected_sha256":sha});
                if !pages.is_empty() {
                    args["pages"] = json!(pages);
                }
                args
            }
            WorkflowDocumentOperation::Create { format, .. } => {
                let (extension, operation) = match format {
                    WorkflowDocumentFormat::Docx => ("docx", "create"),
                    WorkflowDocumentFormat::Pdf => ("pdf", "create_pdf"),
                };
                if !path
                    .rsplit_once('.')
                    .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(extension))
                {
                    return Err(invalid_document());
                }
                let spec = match format {
                    WorkflowDocumentFormat::Docx => {
                        json!({"format":"docx","markdown":value(names[1])?})
                    }
                    WorkflowDocumentFormat::Pdf => json!({"markdown":value(names[1])?}),
                };
                json!({"operation":operation,"output":{"folder_id":folder_id,"path":path},"spec":spec})
            }
        };
        let tool = ToolCall {
            id: "native-document".into(),
            name: self.operation.tool_name().into(),
            arguments,
        };
        crate::documents::parse_document_call(&tool)?;
        Ok(tool)
    }
    fn validate(&self, config: &WorkflowStepData) -> Result<()> {
        valid_text(&self.name, 200)?;
        self.operation.validate()?;
        self.rights.validate()?;
        validate_budgets(&self.budgets)?;
        if !self.rights.tools.contains(self.operation.tool_name())
            || config.output != self.operation.output_type()
            || config.inputs.len() != self.operation.input_names().len()
            || self.operation.input_types().iter().any(|(name, kind)| {
                config
                    .inputs
                    .get(name)
                    .is_none_or(|binding| binding.value_type != *kind)
            })
        {
            return Err(invalid_document());
        }
        match &self.folder {
            WorkflowDocumentFolder::CurrentWorkspace if !self.rights.current_workspace => {
                return Err(invalid_document());
            }
            WorkflowDocumentFolder::SelectedFolder { folder_id } => {
                valid_text(folder_id, 128)?;
                if !self.rights.folders.contains(folder_id) {
                    return Err(invalid_document());
                }
            }
            _ => {}
        }
        if matches!(self.folder, WorkflowDocumentFolder::SelectedFolder { .. })
            && config
                .inputs
                .values()
                .all(|binding| matches!(binding.value, WorkflowDataSource::Literal { .. }))
        {
            let frame = Value::Object(
                config
                    .inputs
                    .iter()
                    .map(|(name, binding)| {
                        let WorkflowDataSource::Literal { value } = &binding.value else {
                            unreachable!("checked all literals");
                        };
                        (name.clone(), value.clone())
                    })
                    .collect(),
            );
            self.tool_call(&frame)?;
        }
        Ok(())
    }
}

pub(super) fn add_nodes(
    definition: &WorkflowDefinition,
    scope: Option<&WorkflowScope>,
    nodes: &mut BTreeMap<String, WorkflowNodePlan>,
) -> Result<()> {
    if definition.documents.is_empty() {
        return Ok(());
    }
    if definition.format_version != 3 || !matches!(definition.mode, WorkflowMode::Parallel { .. }) {
        return Err(invalid_document());
    }
    let data = definition.dataflow.as_ref().ok_or_else(invalid_document)?;
    for (id, step) in &definition.documents {
        identifier(id)?;
        step.validate(data.steps.get(id).ok_or_else(invalid_document)?)?;
        let dependencies: BTreeSet<_> = step.dependencies.iter().cloned().collect();
        if id.starts_with("delegated-")
            || dependencies.len() != step.dependencies.len()
            || nodes.contains_key(id)
        {
            return Err(invalid_document());
        }
        let (rights, budgets) = if let Some(scope) = scope {
            let mut rights = definition
                .rights
                .intersect(&scope.rights)
                .intersect(&step.rights);
            let WorkflowDocumentFolder::SelectedFolder { folder_id } = &step.folder else {
                return Err(Error::Forbidden);
            };
            if !rights.tools.contains(step.operation.tool_name())
                || !rights.folders.contains(folder_id)
            {
                return Err(Error::Forbidden);
            }
            rights
                .tools
                .retain(|tool| tool == step.operation.tool_name());
            rights.folders.retain(|id| id == folder_id);
            (
                rights,
                minimum_budgets(
                    &minimum_budgets(&definition.budgets, &scope.budgets),
                    &step.budgets,
                ),
            )
        } else {
            (step.rights.clone(), step.budgets.clone())
        };
        nodes.insert(
            id.clone(),
            WorkflowNodePlan {
                id: id.clone(),
                role_id: None,
                goal: step.name.clone(),
                dependencies,
                rights,
                budgets,
                depth: 0,
                parent: None,
            },
        );
    }
    Ok(())
}

impl WorkflowDefinition {
    pub fn uses_document_steps(&self) -> bool {
        !self.documents.is_empty()
            || self
                .subflows
                .values()
                .any(|call| call.definition.uses_document_steps())
            || self
                .iterations
                .values()
                .any(|call| call.definition.uses_document_steps())
    }
}

impl WorkflowDefinition {
    /// Resolves data only. The runtime must separately check original COW authority.
    pub fn document_tool_call(
        &self,
        node: &str,
        input: Option<&Value>,
        completed: &BTreeMap<String, String>,
    ) -> Result<ToolCall> {
        let step = self.documents.get(node).ok_or(Error::NotFound)?;
        step.tool_call(&self.resolve_step_data(node, input, completed)?)
    }
}

pub(super) fn used_roles(definition: &WorkflowDefinition) -> BTreeSet<String> {
    let mut selected: BTreeSet<_> = definition
        .steps
        .iter()
        .map(|step| step.role_id.clone())
        .collect();
    loop {
        let before = selected.len();
        for role in &definition.roles {
            if selected.contains(&role.id) {
                selected.extend(role.delegate_to.iter().cloned());
            }
        }
        if selected.len() == before {
            return selected;
        }
    }
}
