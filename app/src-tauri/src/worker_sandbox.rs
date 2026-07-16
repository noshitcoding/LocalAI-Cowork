use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_COPIED_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SANDBOX_ID_LEN: usize = 128;
const IGNORED_DIR_NAMES: [&str; 11] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    "coverage",
    ".cache",
    "venv",
    ".venv",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePrepareResult {
    pub sandbox_root: String,
    pub workspace_root: String,
    pub copied_files: u64,
    pub skipped_files: u64,
    pub skipped_dirs: Vec<String>,
}

#[derive(Default)]
struct CopyStats {
    copied_files: u64,
    skipped_files: u64,
    skipped_dirs: HashSet<String>,
}

pub fn validate_sandbox_id(sandbox_id: &str) -> Result<(), String> {
    if sandbox_id.is_empty() || sandbox_id.len() > MAX_SANDBOX_ID_LEN {
        return Err("sandbox id length is invalid".to_string());
    }
    if !sandbox_id
        .bytes()
        .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    {
        return Err("sandbox id contains invalid characters".to_string());
    }
    Ok(())
}

pub fn sandbox_root(app_data_dir: &Path, sandbox_id: &str) -> Result<PathBuf, String> {
    validate_sandbox_id(sandbox_id)?;
    let container_root = app_data_dir.join("worker_sandboxes");
    fs::create_dir_all(&container_root).map_err(|err| err.to_string())?;
    let canonical_container = container_root
        .canonicalize()
        .map_err(|err| err.to_string())?;
    let candidate = canonical_container.join(sandbox_id);
    if !candidate.starts_with(&canonical_container) {
        return Err("sandbox path escapes its container".to_string());
    }
    Ok(candidate)
}

pub fn prepare_workspace_snapshot(
    app_data_dir: &Path,
    sandbox_id: &str,
    source_root: &Path,
) -> Result<WorkspacePrepareResult, String> {
    let sandbox_root = sandbox_root(app_data_dir, sandbox_id)?;
    let workspace_root = sandbox_root.join("workspace");

    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root).map_err(|err| err.to_string())?;
    }
    if !sandbox_id
        .bytes()
        .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    {
        return Err("sandbox id contains invalid characters".to_string());
    }
    Ok(())
}

pub fn sandbox_root(app_data_dir: &Path, sandbox_id: &str) -> Result<PathBuf, String> {
    validate_sandbox_id(sandbox_id)?;
    let container_root = app_data_dir.join("worker_sandboxes");
    fs::create_dir_all(&container_root).map_err(|err| err.to_string())?;
    let canonical_container = container_root
        .canonicalize()
        .map_err(|err| err.to_string())?;
    let candidate = canonical_container.join(sandbox_id);
    if !candidate.starts_with(&canonical_container) {
        return Err("sandbox path escapes its container".to_string());
    }
    Ok(candidate)
}

pub fn prepare_workspace_snapshot(
    app_data_dir: &Path,
    sandbox_id: &str,
    source_root: &Path,
) -> Result<WorkspacePrepareResult, String> {
    let sandbox_root = sandbox_root(app_data_dir, sandbox_id)?;
    let workspace_root = sandbox_root.join("workspace");

    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root).map_err(|err| err.to_string())?;
    }

    fs::create_dir_all(&workspace_root).map_err(|err| err.to_string())?;

    let mut stats = CopyStats::default();
    copy_dir_recursive(source_root, &workspace_root, &mut stats)?;

    Ok(WorkspacePrepareResult {
        sandbox_root: sandbox_root.display().to_string(),
        workspace_root: workspace_root.display().to_string(),
        copied_files: stats.copied_files,
        skipped_files: stats.skipped_files,
        skipped_dirs: {
            let mut names = stats.skipped_dirs.into_iter().collect::<Vec<_>>();
            names.sort();
            names
        },
    })
}

pub fn destroy_workspace_snapshot(app_data_dir: &Path, sandbox_id: &str) -> Result<(), String> {
    let sandbox_root = sandbox_root(app_data_dir, sandbox_id)?;
    if sandbox_root.exists() {
        fs::remove_dir_all(sandbox_root).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn copy_dir_recursive(
    source: &Path,
    destination: &Path,
    stats: &mut CopyStats,
) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let source_path = entry.path();
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        let target_path = destination.join(entry.file_name());

        if file_type.is_symlink() {
            stats.skipped_files += 1;
            continue;
        }

        if file_type.is_dir() {
            let dir_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            if should_ignore_dir(&dir_name) {
                stats.skipped_dirs.insert(dir_name);
                continue;
            }

            fs::create_dir_all(&target_path).map_err(|err| err.to_string())?;
            copy_dir_recursive(&source_path, &target_path, stats)?;
            continue;
        }

        if file_type.is_file() {
            let metadata = entry.metadata().map_err(|err| err.to_string())?;
            if metadata.len() > MAX_COPIED_FILE_BYTES {
                stats.skipped_files += 1;
                continue;
            }
            fs::copy(&source_path, &target_path).map_err(|err| err.to_string())?;
            stats.copied_files += 1;
        }
    }

    Ok(())
}

fn should_ignore_dir(name: &str) -> bool {
    IGNORED_DIR_NAMES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "open-cowork-worker-sandbox-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("test root should be created");
        root
    }

    #[test]
    fn sandbox_ids_are_opaque_path_safe_identifiers() {
        assert!(validate_sandbox_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(validate_sandbox_id("agent_01").is_ok());
        assert!(validate_sandbox_id("").is_err());
        assert!(validate_sandbox_id("..").is_err());
        assert!(validate_sandbox_id("../outside").is_err());
        assert!(validate_sandbox_id("folder\\outside").is_err());
        assert!(validate_sandbox_id(&"a".repeat(MAX_SANDBOX_ID_LEN + 1)).is_err());
    }

    #[test]
    fn sandbox_root_stays_inside_canonical_container() {
        let app_data = test_root("root");
        let container = app_data.join("worker_sandboxes");
        let resolved = sandbox_root(&app_data, "sandbox-01").expect("safe id should resolve");
        let canonical_container = container.canonicalize().unwrap();

        assert!(resolved.starts_with(&canonical_container));
        assert_eq!(resolved, canonical_container.join("sandbox-01"));

        let _ = fs::remove_dir_all(app_data);
    }

    #[test]
    fn traversal_id_cannot_delete_outside_directory() {
        let parent = test_root("destroy");
        let app_data = parent.join("app-data");
        let victim = parent.join("victim");
        fs::create_dir_all(&app_data).unwrap();
        fs::create_dir_all(&victim).unwrap();
        let marker = victim.join("keep.txt");
        fs::write(&marker, "keep").unwrap();

        assert!(destroy_workspace_snapshot(&app_data, "../victim").is_err());
        assert!(marker.exists());

        let _ = fs::remove_dir_all(parent);
    }
}
