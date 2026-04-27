use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const DELETE_CONFIRM_TOKEN: &str = "DELETE";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWriteResponse {
    pub path: String,
    pub backup_path: Option<String>,
    pub diff: String,
    pub bytes_written: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryCreateResponse {
    pub path: String,
    pub created: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathMutationResponse {
    pub source_path: String,
    pub destination_path: String,
    pub item_kind: String,
    pub created_parent: bool,
    pub replaced_existing: bool,
}

fn canonicalize_for_policy(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|err| err.to_string());
    }

    let mut missing_segments: Vec<PathBuf> = Vec::new();
    let mut existing_ancestor = path;

    while !existing_ancestor.exists() {
        let file_name = existing_ancestor
            .file_name()
            .ok_or_else(|| "path has no existing ancestor".to_string())?;
        missing_segments.push(PathBuf::from(file_name));
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or_else(|| "path has no parent directory".to_string())?;
    }

    let mut canonical = existing_ancestor
        .canonicalize()
        .map_err(|err| err.to_string())?;

    for segment in missing_segments.iter().rev() {
        canonical.push(segment);
    }

    Ok(canonical)
}

pub fn ensure_path_allowed(path: &Path, allowed_folders: &[String]) -> Result<PathBuf, String> {
    if allowed_folders.is_empty() {
        return Err("no allowed folders configured".to_string());
    }

    let canonical_target = canonicalize_for_policy(path)?;

    for folder in allowed_folders {
        let canonical_folder = Path::new(folder)
            .canonicalize()
            .map_err(|err| err.to_string())?;
        if canonical_target.starts_with(&canonical_folder) {
            return Ok(canonical_target);
        }
    }

    Err("path is outside allowed folders".to_string())
}

fn create_backup_path(app_data_dir: &Path, target: &Path) -> Result<PathBuf, String> {
    let backup_root = backup_root(app_data_dir)?;

    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");

    let sanitized = file_name.replace(' ', "_");
    let stamp = Utc::now().format("%Y%m%dT%H%M%S").to_string();

    Ok(backup_root.join(format!("{}_{}.bak", stamp, sanitized)))
}

fn backup_root(app_data_dir: &Path) -> Result<PathBuf, String> {
    let mut backup_root = app_data_dir.to_path_buf();
    backup_root.push("backups");
    fs::create_dir_all(&backup_root).map_err(|err| err.to_string())?;

    Ok(backup_root)
}

fn simple_diff(previous: &str, next: &str) -> String {
    if previous == next {
        return "No changes".to_string();
    }

    let before_lines: Vec<&str> = previous.lines().collect();
    let after_lines: Vec<&str> = next.lines().collect();
    let max_len = before_lines.len().max(after_lines.len());

    let mut output = String::new();
    output.push_str("--- before\n+++ after\n");

    for idx in 0..max_len {
        let before = before_lines.get(idx).copied();
        let after = after_lines.get(idx).copied();

        match (before, after) {
            (Some(left), Some(right)) if left == right => {
                output.push_str(&format!(" {}\n", left));
            }
            (Some(left), Some(right)) => {
                output.push_str(&format!("-{}\n", left));
                output.push_str(&format!("+{}\n", right));
            }
            (Some(left), None) => output.push_str(&format!("-{}\n", left)),
            (None, Some(right)) => output.push_str(&format!("+{}\n", right)),
            (None, None) => {}
        }
    }

    output
}

pub fn write_text_file(
    app_data_dir: &Path,
    canonical_target: &Path,
    content: &str,
    create_backup: bool,
) -> Result<FileWriteResponse, String> {
    if let Some(parent) = canonical_target.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    let previous = if canonical_target.exists() {
        fs::read_to_string(canonical_target).unwrap_or_default()
    } else {
        String::new()
    };

    let backup_path = if create_backup && canonical_target.exists() {
        let backup = create_backup_path(app_data_dir, canonical_target)?;
        fs::copy(canonical_target, &backup).map_err(|err| err.to_string())?;
        Some(backup)
    } else {
        None
    };

    fs::write(canonical_target, content).map_err(|err| err.to_string())?;

    Ok(FileWriteResponse {
        path: canonical_target.display().to_string(),
        backup_path: backup_path.map(|value| value.display().to_string()),
        diff: simple_diff(&previous, content),
        bytes_written: content.len(),
    })
}

pub fn delete_file(canonical_target: &Path, confirm_token: &str) -> Result<(), String> {
    if confirm_token != DELETE_CONFIRM_TOKEN {
        return Err("delete confirmation token mismatch".to_string());
    }

    if !canonical_target.exists() {
        return Err("target does not exist".to_string());
    }

    fs::remove_file(canonical_target).map_err(|err| err.to_string())
}

pub fn create_directory(canonical_target: &Path) -> Result<DirectoryCreateResponse, String> {
    if canonical_target.exists() {
        if !canonical_target.is_dir() {
            return Err("target exists and is not a directory".to_string());
        }

        return Ok(DirectoryCreateResponse {
            path: canonical_target.display().to_string(),
            created: false,
        });
    }

    fs::create_dir_all(canonical_target).map_err(|err| err.to_string())?;

    Ok(DirectoryCreateResponse {
        path: canonical_target.display().to_string(),
        created: true,
    })
}

fn remove_path_if_exists(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }

    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|err| err.to_string())?;
    } else {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }

    Ok(true)
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|err| err.to_string())?;

    for entry in fs::read_dir(source).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|err| err.to_string())?;

        if file_type.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
            continue;
        }

        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }

        fs::copy(&source_path, &destination_path).map_err(|err| err.to_string())?;
    }

    Ok(())
}

pub fn copy_path(
    canonical_source: &Path,
    canonical_destination: &Path,
    overwrite: bool,
) -> Result<PathMutationResponse, String> {
    if !canonical_source.exists() {
        return Err("source path does not exist".to_string());
    }

    let created_parent = match canonical_destination.parent() {
        Some(parent) if !parent.exists() => {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            true
        }
        Some(_) => false,
        None => false,
    };

    if canonical_destination.exists() && !overwrite {
        return Err("destination already exists".to_string());
    }

    let replaced_existing = if overwrite {
        remove_path_if_exists(canonical_destination)?
    } else {
        false
    };

    let item_kind = if canonical_source.is_dir() {
        copy_directory_recursive(canonical_source, canonical_destination)?;
        "directory"
    } else {
        fs::copy(canonical_source, canonical_destination).map_err(|err| err.to_string())?;
        "file"
    };

    Ok(PathMutationResponse {
        source_path: canonical_source.display().to_string(),
        destination_path: canonical_destination.display().to_string(),
        item_kind: item_kind.to_string(),
        created_parent,
        replaced_existing,
    })
}

pub fn move_path(
    canonical_source: &Path,
    canonical_destination: &Path,
    overwrite: bool,
) -> Result<PathMutationResponse, String> {
    if !canonical_source.exists() {
        return Err("source path does not exist".to_string());
    }

    let created_parent = match canonical_destination.parent() {
        Some(parent) if !parent.exists() => {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            true
        }
        Some(_) => false,
        None => false,
    };

    if canonical_destination.exists() && !overwrite {
        return Err("destination already exists".to_string());
    }

    let replaced_existing = if overwrite {
        remove_path_if_exists(canonical_destination)?
    } else {
        false
    };

    let item_kind = if canonical_source.is_dir() {
        "directory"
    } else {
        "file"
    };

    match fs::rename(canonical_source, canonical_destination) {
        Ok(_) => {}
        Err(_) if canonical_source.is_dir() => {
            copy_directory_recursive(canonical_source, canonical_destination)?;
            fs::remove_dir_all(canonical_source).map_err(|err| err.to_string())?;
        }
        Err(_) => {
            fs::copy(canonical_source, canonical_destination).map_err(|err| err.to_string())?;
            fs::remove_file(canonical_source).map_err(|err| err.to_string())?;
        }
    }

    Ok(PathMutationResponse {
        source_path: canonical_source.display().to_string(),
        destination_path: canonical_destination.display().to_string(),
        item_kind: item_kind.to_string(),
        created_parent,
        replaced_existing,
    })
}

pub fn list_backups(app_data_dir: &Path) -> Result<Vec<BackupEntry>, String> {
    let backup_root = backup_root(app_data_dir)?;
    let mut entries = Vec::new();

    for entry in fs::read_dir(&backup_root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let metadata = entry.metadata().map_err(|err| err.to_string())?;
        let modified_at = metadata
            .modified()
            .ok()
            .map(chrono::DateTime::<Utc>::from)
            .map(|value| value.to_rfc3339());

        entries.push(BackupEntry {
            file_name: entry.file_name().to_string_lossy().to_string(),
            path: path.display().to_string(),
            size_bytes: metadata.len(),
            modified_at,
        });
    }

    entries.sort_by(|left, right| right.file_name.cmp(&left.file_name));
    Ok(entries)
}

pub fn restore_backup(
    app_data_dir: &Path,
    backup_file_name: &str,
    target: &Path,
    create_backup: bool,
) -> Result<FileWriteResponse, String> {
    let backup_root = backup_root(app_data_dir)?;
    let backup_path = backup_root.join(backup_file_name);
    if !backup_path.exists() {
        return Err("backup file not found".to_string());
    }

    let content = fs::read_to_string(&backup_path).map_err(|err| err.to_string())?;
    write_text_file(app_data_dir, target, &content, create_backup)
}

pub fn write_file_audit_details(path: &str, backup_path: Option<&str>, bytes: usize) -> serde_json::Value {
    json!({
        "path": path,
        "backupPath": backup_path,
        "bytes": bytes,
    })
}

pub fn delete_file_audit_details(path: &str) -> serde_json::Value {
    json!({
        "path": path,
    })
}

pub fn restore_file_audit_details(path: &str, backup_file_name: &str) -> serde_json::Value {
    json!({
        "path": path,
        "backupFileName": backup_file_name,
    })
}

pub fn create_directory_audit_details(path: &str, created: bool) -> serde_json::Value {
    json!({
        "path": path,
        "created": created,
    })
}

pub fn mutate_path_audit_details(
    operation: &str,
    source_path: &str,
    destination_path: &str,
    item_kind: &str,
    created_parent: bool,
    replaced_existing: bool,
) -> serde_json::Value {
    json!({
        "operation": operation,
        "sourcePath": source_path,
        "destinationPath": destination_path,
        "itemKind": item_kind,
        "createdParent": created_parent,
        "replacedExisting": replaced_existing,
    })
}
