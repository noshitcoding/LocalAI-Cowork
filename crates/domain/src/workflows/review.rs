//! Frozen typed human review. Values and answers never grant execution rights.
use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewFormat {
    Value,
    Date,
    MoneyMinor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewField {
    pub label: String,
    pub help: String,
    pub value_type: WorkflowDataType,
    pub format: WorkflowReviewFormat,
    pub required: bool,
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowReviewCheck {
    Sum { terms: Vec<String>, total: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewForm {
    pub question: String,
    pub fields: BTreeMap<String, WorkflowReviewField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_steps: Vec<String>,
    pub checks: Vec<WorkflowReviewCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewEvidenceKind {
    Text,
    Ocr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewEvidence {
    pub step_id: String,
    pub name: String,
    pub kind: WorkflowReviewEvidenceKind,
    pub output: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewRequest {
    pub form: WorkflowReviewForm,
    pub values: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<WorkflowReviewEvidence>,
}

fn invalid_review() -> Error {
    Error::Validation("workflow_review_invalid".into())
}
fn valid_date(text: &str) -> bool {
    if text.len() != 10
        || !text.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return false;
    }
    let year = text[..4].parse::<u32>().unwrap_or(0);
    let month = text[5..7].parse::<usize>().unwrap_or(0);
    let day = text[8..].parse::<u32>().unwrap_or(0);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        0,
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year > 0 && (1..=12).contains(&month) && day > 0 && day <= days[month]
}
fn base_type(kind: &WorkflowDataType) -> &WorkflowDataType {
    if let WorkflowDataType::Nullable { value } = kind {
        value
    } else {
        kind
    }
}

impl WorkflowReviewForm {
    pub fn prefill_type(&self) -> WorkflowDataType {
        WorkflowDataType::Object {
            fields: self
                .fields
                .iter()
                .map(|(name, field)| (name.clone(), field.value_type.clone()))
                .collect(),
        }
    }
    pub fn output_type(&self) -> WorkflowDataType {
        WorkflowDataType::Object {
            fields: self
                .fields
                .iter()
                .map(|(name, field)| {
                    (
                        name.clone(),
                        if field.required {
                            base_type(&field.value_type).clone()
                        } else {
                            field.value_type.clone()
                        },
                    )
                })
                .collect(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        valid_text(&self.question, 4000)?;
        if self.fields.is_empty()
            || self.fields.len() > 32
            || self.checks.len() > 16
            || self.source_steps.len() > 4
            || self.source_steps.iter().collect::<BTreeSet<_>>().len() != self.source_steps.len()
            || self
                .source_steps
                .iter()
                .any(|source| valid_text(source, 120).is_err())
        {
            return Err(invalid_review());
        }
        self.prefill_type().validate_type()?;
        self.output_type().validate_type()?;
        for field in self.fields.values() {
            valid_text(&field.label, 200)?;
            if field.help.len() > 800 || field.help.contains('\0') {
                return Err(invalid_review());
            }
            let kind = base_type(&field.value_type);
            if field.required && kind == &WorkflowDataType::Null
                || matches!(field.format, WorkflowReviewFormat::Date)
                    && kind != &WorkflowDataType::Text
                || matches!(field.format, WorkflowReviewFormat::MoneyMinor)
                    && kind != &WorkflowDataType::Integer
                || (field.minimum.is_some() || field.maximum.is_some())
                    && kind != &WorkflowDataType::Integer
                || field
                    .minimum
                    .is_some_and(|v| v.abs_diff(0) > 9_007_199_254_740_991)
                || field
                    .maximum
                    .is_some_and(|v| v.abs_diff(0) > 9_007_199_254_740_991)
                || matches!((field.minimum,field.maximum),(Some(min),Some(max)) if min>max)
            {
                return Err(invalid_review());
            }
        }
        for check in &self.checks {
            let WorkflowReviewCheck::Sum { terms, total } = check;
            let distinct: BTreeSet<_> = terms.iter().collect();
            if !(2..=16).contains(&terms.len())
                || distinct.len() != terms.len()
                || distinct.contains(total)
                || terms.iter().chain(std::iter::once(total)).any(|name| {
                    self.fields.get(name).is_none_or(|field| {
                        (if field.required {
                            base_type(&field.value_type)
                        } else {
                            &field.value_type
                        }) != &WorkflowDataType::Integer
                    })
                })
            {
                return Err(invalid_review());
            }
        }
        Ok(())
    }
    pub fn validate_answer(&self, value: &Value) -> Result<()> {
        self.validate()?;
        self.output_type().validate_value(value)?;
        for (name, field) in &self.fields {
            let value = &value[name];
            if field.required
                && (value.is_null()
                    || value.as_str().is_some_and(|v| v.trim().is_empty())
                    || value.as_array().is_some_and(Vec::is_empty))
            {
                return Err(Error::Validation(format!(
                    "workflow_review_required:{name}"
                )));
            }
            if value.is_null() {
                continue;
            }
            if matches!(field.format, WorkflowReviewFormat::Date) {
                let text = value.as_str().ok_or_else(invalid_review)?;
                if !valid_date(text) {
                    return Err(Error::Validation(format!("workflow_review_date:{name}")));
                }
            }
            if field.minimum.is_some() || field.maximum.is_some() {
                let integer = value.as_f64().ok_or_else(invalid_review)? as i64;
                if field.minimum.is_some_and(|min| integer < min)
                    || field.maximum.is_some_and(|max| integer > max)
                {
                    return Err(Error::Validation(format!("workflow_review_range:{name}")));
                }
            }
        }
        for check in &self.checks {
            let WorkflowReviewCheck::Sum { terms, total } = check;
            let integer = |name: &str| {
                value[name]
                    .as_f64()
                    .map(|v| v as i64)
                    .ok_or_else(invalid_review)
            };
            let sum = terms.iter().try_fold(0_i64, |sum, name| {
                sum.checked_add(integer(name)?).ok_or_else(invalid_review)
            })?;
            if sum != integer(total)? {
                return Err(Error::Validation(format!("workflow_review_sum:{total}")));
            }
        }
        Ok(())
    }
}

impl WorkflowDefinition {
    pub fn review_form(&self, node: &str) -> Option<&WorkflowReviewForm> {
        match self.controls.get(node).map(|step| &step.operation) {
            Some(WorkflowControlOperation::Review { form, .. }) => Some(form),
            _ => None,
        }
    }
    pub fn review_request(
        &self,
        node: &str,
        input: Option<&Value>,
        completed: &BTreeMap<String, String>,
    ) -> Result<WorkflowReviewRequest> {
        self.validate_definition()?;
        let form = self.review_form(node).ok_or(Error::Forbidden)?;
        form.validate()?;
        let values = self.resolve_step_data(node, input, completed)?;
        form.prefill_type().validate_value(&values)?;
        let mut evidence = Vec::new();
        let mut bytes = 0_usize;
        for source in &form.source_steps {
            let document = self.documents.get(source).ok_or_else(invalid_review)?;
            let kind = match document.operation {
                WorkflowDocumentOperation::ReadText { .. } => WorkflowReviewEvidenceKind::Text,
                WorkflowDocumentOperation::Ocr { .. } => WorkflowReviewEvidenceKind::Ocr,
                _ => return Err(invalid_review()),
            };
            let output = self.decode_step_output(
                source,
                completed.get(source).ok_or_else(|| {
                    Error::Validation("workflow_review_source_unavailable".into())
                })?,
            )?;
            bytes = bytes.saturating_add(
                serde_json::to_vec(&output)
                    .map_err(|_| invalid_review())?
                    .len(),
            );
            if bytes > 128_000 {
                return Err(Error::Validation("workflow_review_source_limit".into()));
            }
            evidence.push(WorkflowReviewEvidence {
                step_id: source.clone(),
                name: document.name.clone(),
                kind,
                output,
            });
        }
        Ok(WorkflowReviewRequest {
            form: form.clone(),
            values,
            evidence,
        })
    }
}
