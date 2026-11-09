//! Data-only table schemas. This module never reads or publishes workbooks.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTableColumnKind {
    Text,
    Integer,
    Number,
    Boolean,
    Date,
    MoneyMinor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTableColumn {
    pub field: String,
    pub header: String,
    pub kind: WorkflowTableColumnKind,
    pub nullable: bool,
}

pub(super) fn rows_type(columns: &[WorkflowTableColumn]) -> WorkflowDataType {
    WorkflowDataType::List {
        max_items: 100,
        item: Box::new(WorkflowDataType::Object {
            fields: columns
                .iter()
                .map(|column| {
                    let kind = match column.kind {
                        WorkflowTableColumnKind::Text | WorkflowTableColumnKind::Date => {
                            WorkflowDataType::Text
                        }
                        WorkflowTableColumnKind::Integer | WorkflowTableColumnKind::MoneyMinor => {
                            WorkflowDataType::Integer
                        }
                        WorkflowTableColumnKind::Number => WorkflowDataType::Number,
                        WorkflowTableColumnKind::Boolean => WorkflowDataType::Boolean,
                    };
                    (
                        column.field.clone(),
                        if column.nullable {
                            WorkflowDataType::Nullable {
                                value: Box::new(kind),
                            }
                        } else {
                            kind
                        },
                    )
                })
                .collect(),
        }),
    }
}

pub(super) fn validate_schema(
    sheet: &str,
    table: &str,
    columns: &[WorkflowTableColumn],
    keys: &[String],
) -> Result<()> {
    valid_text(sheet, 31)?;
    valid_text(table, 255)?;
    if columns.is_empty()
        || columns.len() > 32
        || keys.is_empty()
        || keys.len() > 4
        || keys.iter().collect::<BTreeSet<_>>().len() != keys.len()
    {
        return Err(invalid_document());
    }
    let mut fields = BTreeSet::new();
    let mut headers = BTreeSet::new();
    for column in columns {
        identifier(&column.field)?;
        valid_text(&column.header, 255)?;
        if !fields.insert(&column.field) || !headers.insert(column.header.to_lowercase()) {
            return Err(invalid_document());
        }
    }
    if keys.iter().any(|key| {
        !columns
            .iter()
            .any(|column| column.field == *key && !column.nullable)
    }) {
        return Err(invalid_document());
    }
    rows_type(columns).validate_type()
}

pub(super) fn output_type() -> WorkflowDataType {
    use WorkflowDataType::{Integer, Text};
    object(&[
        ("source", source_type()),
        (
            "file",
            object(&[
                ("folder_id", Text),
                ("path", Text),
                ("sha256", Text),
                ("mime_type", Text),
                ("bytes", Integer),
            ]),
        ),
        (
            "table",
            object(&[
                ("sheet", Text),
                ("table", Text),
                ("rows_added", Integer),
                ("first_row", Integer),
                ("last_row", Integer),
                ("previous_range", Text),
                ("range", Text),
                (
                    "key_columns",
                    WorkflowDataType::List {
                        item: Box::new(Integer),
                        max_items: 4,
                    },
                ),
            ]),
        ),
    ])
}

pub(super) fn patch(
    sheet: &str,
    table: &str,
    columns: &[WorkflowTableColumn],
    keys: &[String],
    rows: &Value,
) -> Result<Value> {
    let rows = rows
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 100)
        .ok_or_else(invalid_document)?;
    let mut prepared = vec![];
    let mut seen = BTreeSet::new();
    for row in rows {
        let row = row
            .as_object()
            .filter(|row| row.len() == columns.len())
            .ok_or_else(invalid_document)?;
        let values = columns
            .iter()
            .map(|column| {
                let value = row.get(&column.field).ok_or_else(invalid_document)?;
                if value.is_null() && column.nullable {
                    return Ok(value.clone());
                }
                match column.kind {
                    WorkflowTableColumnKind::Text => {
                        let text = value
                            .as_str()
                            .filter(|text| !text.is_empty())
                            .ok_or_else(invalid_document)?;
                        Ok(json!(text))
                    }
                    WorkflowTableColumnKind::Date => {
                        let text = value.as_str().ok_or_else(invalid_document)?;
                        let parts: Vec<_> = text.split('-').collect();
                        let year = parts
                            .first()
                            .and_then(|part| part.parse::<u32>().ok())
                            .unwrap_or(0);
                        let month = parts
                            .get(1)
                            .and_then(|part| part.parse::<usize>().ok())
                            .unwrap_or(0);
                        let day = parts
                            .get(2)
                            .and_then(|part| part.parse::<u32>().ok())
                            .unwrap_or(0);
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
                        if parts.len() != 3
                            || text.len() != 10
                            || parts
                                .iter()
                                .any(|part| !part.bytes().all(|b| b.is_ascii_digit()))
                            || parts[0].len() != 4
                            || parts[1].len() != 2
                            || parts[2].len() != 2
                            || !(1900..=9999).contains(&year)
                            || !(1..=12).contains(&month)
                            || day == 0
                            || day > days[month]
                        {
                            return Err(invalid_document());
                        }
                        Ok(json!(text))
                    }
                    WorkflowTableColumnKind::Integer | WorkflowTableColumnKind::MoneyMinor => {
                        let number = value
                            .as_f64()
                            .filter(|number| number.is_finite() && number.fract() == 0.0)
                            .ok_or_else(invalid_document)?;
                        let bound = if column.kind == WorkflowTableColumnKind::MoneyMinor {
                            9_999_999_999_999.0
                        } else {
                            999_999_999_999_999.0
                        };
                        if number.abs() > bound {
                            return Err(invalid_document());
                        }
                        Ok(json!(number as i64))
                    }
                    WorkflowTableColumnKind::Boolean if value.is_boolean() => Ok(value.clone()),
                    WorkflowTableColumnKind::Number if value.is_number() => Ok(value.clone()),
                    _ => Err(invalid_document()),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let key = keys
            .iter()
            .map(|key| {
                let index = columns
                    .iter()
                    .position(|column| column.field == *key)
                    .ok_or_else(invalid_document)?;
                if values[index]
                    .as_str()
                    .is_some_and(|text| text.trim().is_empty())
                {
                    return Err(invalid_document());
                }
                Ok(values[index].clone())
            })
            .collect::<Result<Vec<_>>>()?;
        if !seen.insert(canonical_hash(&json!(key))) {
            return Err(Error::Validation("workflow_table_duplicate_key".into()));
        }
        prepared.push(values);
    }
    let key_columns = keys
        .iter()
        .map(|key| {
            columns
                .iter()
                .position(|column| column.field == *key)
                .ok_or_else(invalid_document)
        })
        .collect::<Result<Vec<_>>>()?;
    let schema: Vec<_> = columns
        .iter()
        .map(|column| json!({"header":column.header,"kind":column.kind,"nullable":column.nullable}))
        .collect();
    Ok(
        json!({"op":"append_table_rows","sheet":sheet,"table":table,"columns":schema,"key_columns":key_columns,"rows":prepared}),
    )
}
