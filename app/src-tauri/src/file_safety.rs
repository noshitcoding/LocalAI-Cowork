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
    if !path.is_absolute() {
        return Err("path must be absolute".to_string());
    }

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
    let mut valid_root_count = 0_usize;

    for folder in allowed_folders {
        let folder_path = Path::new(folder);
        if !folder_path.is_absolute() {
            continue;
        }
        let Ok(canonical_folder) = folder_path.canonicalize() else {
            continue;
        };
        if !canonical_folder.is_dir() {
            continue;
        }
        valid_root_count += 1;
        if canonical_target.starts_with(&canonical_folder) {
            return Ok(canonical_target);
        }
    }

    if valid_root_count == 0 {
        return Err("no valid allowed folders configured".to_string());
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

        if file_type.is_symlink() {
            return Err(format!(
                "symbolic links and junctions are not copied: {}",
                source_path.display()
            ));
        }

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
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        if file_type.is_symlink() || !file_type.is_file() {
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
    let requested_name = Path::new(backup_file_name);
    let is_single_file_name = requested_name.components().count() == 1
        && requested_name
            .file_name()
            .is_some_and(|value| value == requested_name.as_os_str());
    if !is_single_file_name || backup_file_name.trim().is_empty() {
        return Err("backup_file_name must be a single file name".to_string());
    }

    let canonical_backup_root = backup_root.canonicalize().map_err(|err| err.to_string())?;
    let backup_path = canonical_backup_root.join(requested_name);
    let canonical_backup_path = backup_path
        .canonicalize()
        .map_err(|_| "backup file not found".to_string())?;
    if !canonical_backup_path.starts_with(&canonical_backup_root)
        || !canonical_backup_path.is_file()
    {
        return Err("backup file not found".to_string());
    }

    let content = fs::read_to_string(&canonical_backup_path).map_err(|err| err.to_string())?;
    write_text_file(app_data_dir, target, &content, create_backup)
}

pub fn write_file_audit_details(
    path: &str,
    backup_path: Option<&str>,
    bytes: usize,
) -> serde_json::Value {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use std::process::{Command, Stdio};

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "open-cowork-file-safety-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("test root should be created");
        root
    }

    fn allowed(root: &Path) -> Vec<String> {
        vec![root.display().to_string()]
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("directory symlink should be created");
    }

    #[cfg(target_os = "windows")]
    fn create_directory_link(target: &Path, link: &Path) {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let cmd = PathBuf::from(system_root).join("System32").join("cmd.exe");
        let status = Command::new(cmd)
            .args(["/D", "/S", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("mklink should start");
        assert!(status.success(), "directory junction should be created");
    }

    #[cfg(unix)]
    fn remove_directory_link(link: &Path) {
        fs::remove_file(link).expect("directory symlink should be removed");
    }

    #[cfg(target_os = "windows")]
    fn remove_directory_link(link: &Path) {
        fs::remove_dir(link).expect("directory junction should be removed");
    }

    #[test]
    fn path_policy_requires_absolute_targets_and_valid_roots() {
        let root = test_root("absolute");
        let stale_root = root.join("missing-root");
        let target = root.join("nested").join("new.txt");

        assert!(ensure_path_allowed(Path::new("relative.txt"), &allowed(&root)).is_err());
        assert!(ensure_path_allowed(&target, &[stale_root.display().to_string()]).is_err());
        let resolved = ensure_path_allowed(
            &target,
            &[stale_root.display().to_string(), root.display().to_string()],
        )
        .expect("valid roots after stale roots should still be considered");
        assert!(resolved.starts_with(root.canonicalize().unwrap()));
        assert!(resolved.ends_with(Path::new("nested").join("new.txt")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_policy_rejects_sibling_prefixes() {
        let parent = test_root("sibling-prefix");
        let allowed_root = parent.join("workspace");
        let sibling = parent.join("workspace-escape");
        fs::create_dir_all(&allowed_root).expect("allowed root should be created");
        fs::create_dir_all(&sibling).expect("sibling should be created");
        let secret = sibling.join("secret.txt");
        fs::write(&secret, "secret").expect("secret fixture should be written");

        assert!(ensure_path_allowed(&secret, &allowed(&allowed_root)).is_err());

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn junction_escape_and_recursive_copy_are_blocked() {
        let parent = test_root("junction");
        let allowed_root = parent.join("workspace");
        let source = allowed_root.join("source");
        let outside = parent.join("outside");
        let link = source.join("escape");
        fs::create_dir_all(&source).expect("source should be created");
        fs::create_dir_all(&outside).expect("outside directory should be created");
        fs::write(outside.join("secret.txt"), "secret").expect("outside fixture should be written");
        create_directory_link(&outside, &link);

        assert!(ensure_path_allowed(&link.join("secret.txt"), &allowed(&allowed_root)).is_err());

        let destination = allowed_root.join("copy");
        let copy_result = copy_path(&source, &destination, false);
        assert!(copy_result.is_err());
        assert!(!destination.join("escape").join("secret.txt").exists());

        remove_directory_link(&link);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn restore_backup_rejects_path_traversal_and_restores_valid_entries() {
        let parent = test_root("backup");
        let app_data = parent.join("app-data");
        let backups = app_data.join("backups");
        fs::create_dir_all(&backups).expect("backup root should be created");
        fs::write(backups.join("valid.bak"), "restored").expect("valid backup should be written");
        let outside = parent.join("outside.txt");
        fs::write(&outside, "outside-secret").expect("outside fixture should be written");
        let target = parent.join("target.txt");

        assert!(restore_backup(&app_data, "../outside.txt", &target, false).is_err());
        assert!(restore_backup(&app_data, &outside.display().to_string(), &target, false).is_err());
        assert!(!target.exists());

        let response = restore_backup(&app_data, "valid.bak", &target, false)
            .expect("valid backup should restore");
        assert_eq!(fs::read_to_string(&target).unwrap(), "restored");
        assert_eq!(response.path, target.display().to_string());

        let _ = fs::remove_dir_all(parent);
    }
}
