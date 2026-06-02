use chrono::Utc;
use serde_json::{Value, json};
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::path::PathBuf;

pub fn append_audit_event(
    mut app_data_dir: PathBuf,
    area: &str,
    action: &str,
    details: Option<Value>,
) -> Result<(), String> {
    app_data_dir.push("audit");
    create_dir_all(&app_data_dir).map_err(|err| err.to_string())?;

    let mut file_path = app_data_dir;
    file_path.push("events.jsonl");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .map_err(|err| err.to_string())?;

    let event = json!({
      "timestamp": Utc::now().to_rfc3339(),
      "area": area,
      "action": action,
      "details": details,
    });

    writeln!(file, "{}", event).map_err(|err| err.to_string())
}
