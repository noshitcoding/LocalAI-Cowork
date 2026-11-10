use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunManifest {
    pub version: u32,
    pub roots: Vec<ManifestRoot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRoot {
    pub root_id: String,
    pub root_label: String,
    pub source_path: String,
    pub workspace_path: String,
    pub kind: String,
    pub access: String,
    pub files: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug, Clone)]
pub struct SnapshotRootInput {
    pub root_id: String,
    pub root_label: String,
    pub source_path: PathBuf,
    pub kind: String,
    pub access: String,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRootMapping {
    pub root_id: String,
    pub root_label: String,
    pub source_path: String,
    pub workspace_path: String,
    pub kind: String,
    pub access: String,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiWorkspacePrepareResult {
    pub sandbox_root: String,
    pub workspace_root: String,
    pub primary_cwd: String,
    pub copied_files: u64,
    pub skipped_files: u64,
    pub skipped_dirs: Vec<String>,
    pub roots: Vec<WorkspaceRootMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub root_label: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub binary: bool,
    pub preview: Option<String>,
    pub applicable: bool,
    pub policy_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDiff {
    pub sandbox_id: String,
    pub source_root: String,
    pub workspace_root: String,
    pub roots: Vec<WorkspaceRootMapping>,
    pub changes: Vec<FileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub applied: Vec<String>,
    pub conflicts: Vec<String>,
    pub rejected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyRunManifest {
    source_root: String,
    files: BTreeMap<String, ManifestEntry>,
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

pub fn prepare_workspace_snapshot_multi(
    app_data_dir: &Path,
    sandbox_id: &str,
    inputs: &[SnapshotRootInput],
) -> Result<MultiWorkspacePrepareResult, String> {
    let sandbox_root = sandbox_root(app_data_dir, sandbox_id)?;
    let workspace_root = sandbox_root.join("workspace");
    let roots_root = workspace_root.join("roots");

    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&roots_root).map_err(|error| error.to_string())?;

    let mut stats = CopyStats::default();
    let mut mappings = Vec::new();
    for input in inputs {
        validate_sandbox_id(&input.root_id)?;
        if !matches!(input.kind.as_str(), "file" | "folder") {
            return Err(format!("unsupported sandbox root kind: {}", input.kind));
        }
        if !matches!(input.access.as_str(), "read_only" | "read_write") {
            return Err(format!("unsupported sandbox root access: {}", input.access));
        }
        let source = input
            .source_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let root_workspace = roots_root.join(&input.root_id);
        fs::create_dir_all(&root_workspace).map_err(|error| error.to_string())?;
        if input.kind == "folder" {
            if !source.is_dir() {
                return Err(format!(
                    "sandbox folder root is not a directory: {}",
                    source.display()
                ));
            }
            copy_dir_recursive(&source, &root_workspace, &mut stats)?;
        } else {
            if !source.is_file() {
                return Err(format!(
                    "sandbox file root is not a file: {}",
                    source.display()
                ));
            }
            reject_unsafe_file(&source)?;
            let file_name = source
                .file_name()
                .ok_or_else(|| "sandbox file root has no file name".to_string())?;
            fs::copy(&source, root_workspace.join(file_name)).map_err(|error| error.to_string())?;
            stats.copied_files += 1;
        }
        mappings.push(WorkspaceRootMapping {
            root_id: input.root_id.clone(),
            root_label: input.root_label.clone(),
            source_path: source.display().to_string(),
            workspace_path: root_workspace.display().to_string(),
            kind: input.kind.clone(),
            access: input.access.clone(),
            is_primary: input.is_primary,
        });
    }

    let primary_cwd = mappings
        .iter()
        .find(|root| root.is_primary)
        .or_else(|| mappings.iter().find(|root| root.access == "read_write"))
        .or_else(|| mappings.first())
        .map(|root| root.workspace_path.clone())
        .unwrap_or_else(|| workspace_root.display().to_string());
    let manifest = RunManifest {
        version: 2,
        roots: mappings
            .iter()
            .map(|mapping| {
                let workspace = PathBuf::from(&mapping.workspace_path);
                Ok(ManifestRoot {
                    root_id: mapping.root_id.clone(),
                    root_label: mapping.root_label.clone(),
                    source_path: mapping.source_path.clone(),
                    workspace_path: mapping.workspace_path.clone(),
                    kind: mapping.kind.clone(),
                    access: mapping.access.clone(),
                    files: collect_manifest(&workspace)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    write_file_atomically(
        &sandbox_root.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )?;

    Ok(MultiWorkspacePrepareResult {
        sandbox_root: sandbox_root.display().to_string(),
        workspace_root: workspace_root.display().to_string(),
        primary_cwd,
        copied_files: stats.copied_files,
        skipped_files: stats.skipped_files,
        skipped_dirs: {
            let mut names = stats.skipped_dirs.into_iter().collect::<Vec<_>>();
            names.sort();
            names
        },
        roots: mappings,
    })
}

pub fn destroy_workspace_snapshot(app_data_dir: &Path, sandbox_id: &str) -> Result<(), String> {
    let sandbox_root = sandbox_root(app_data_dir, sandbox_id)?;
    if sandbox_root.exists() {
        fs::remove_dir_all(sandbox_root).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn write_run_manifest(
    app_data_dir: &Path,
    sandbox_id: &str,
    source_root: &Path,
) -> Result<(), String> {
    let manifest = RunManifest {
        version: 2,
        roots: vec![ManifestRoot {
            root_id: "workspace".to_string(),
            root_label: source_root
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Workspace".to_string()),
            source_path: source_root.display().to_string(),
            workspace_path: sandbox_root(app_data_dir, sandbox_id)?
                .join("workspace")
                .display()
                .to_string(),
            kind: "folder".to_string(),
            access: "read_write".to_string(),
            files: collect_manifest(source_root)?,
        }],
    };
    let path = sandbox_root(app_data_dir, sandbox_id)?.join("manifest.json");
    fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn load_run_manifest(root: &Path) -> Result<RunManifest, String> {
    let bytes = fs::read(root.join("manifest.json")).map_err(|error| error.to_string())?;
    if let Ok(manifest) = serde_json::from_slice::<RunManifest>(&bytes) {
        if manifest.version == 2 {
            return Ok(manifest);
        }
        return Err(format!(
            "unsupported sandbox manifest version {}",
            manifest.version
        ));
    }
    let legacy: LegacyRunManifest =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(RunManifest {
        version: 2,
        roots: vec![ManifestRoot {
            root_id: "workspace".to_string(),
            root_label: "Workspace".to_string(),
            source_path: legacy.source_root,
            workspace_path: root.join("workspace").display().to_string(),
            kind: "folder".to_string(),
            access: "read_write".to_string(),
            files: legacy.files,
        }],
    })
}

pub fn run_diff(app_data_dir: &Path, sandbox_id: &str) -> Result<RunDiff, String> {
    let root = sandbox_root(app_data_dir, sandbox_id)?;
    let workspace = root.join("workspace");
    let baseline = load_run_manifest(&root)?;
    let mut changes = Vec::new();
    let mappings = baseline
        .roots
        .iter()
        .map(|item| WorkspaceRootMapping {
            root_id: item.root_id.clone(),
            root_label: item.root_label.clone(),
            source_path: item.source_path.clone(),
            workspace_path: item.workspace_path.clone(),
            kind: item.kind.clone(),
            access: item.access.clone(),
            is_primary: false,
        })
        .collect::<Vec<_>>();
    for manifest_root in &baseline.roots {
        let root_workspace = PathBuf::from(&manifest_root.workspace_path);
        let current = collect_manifest(&root_workspace)?;
        let mut paths = manifest_root
            .files
            .keys()
            .chain(current.keys())
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        for relative in paths {
            let before = manifest_root.files.get(&relative);
            let after = current.get(&relative);
            if before == after {
                continue;
            }
            let kind = if before.is_none() {
                "created"
            } else if after.is_none() {
                "deleted"
            } else {
                "modified"
            };
            let candidate = root_workspace.join(relative_path(&relative)?);
            let (size, binary, preview) = if let Some(entry) = after {
                let bytes = fs::read(&candidate).map_err(|error| error.to_string())?;
                let binary = bytes.iter().take(8192).any(|byte| *byte == 0)
                    || std::str::from_utf8(&bytes).is_err();
                let preview = if binary {
                    None
                } else {
                    Some(String::from_utf8_lossy(&bytes[..bytes.len().min(12_000)]).into_owned())
                };
                (entry.size, binary, preview)
            } else {
                (before.map(|entry| entry.size).unwrap_or(0), false, None)
            };
            let file_root_name = Path::new(&manifest_root.source_path)
                .file_name()
                .map(|value| value.to_string_lossy().replace('\\', "/"));
            let file_root_violation = manifest_root.kind == "file"
                && file_root_name.as_deref() != Some(relative.as_str());
            let policy_error = if manifest_root.access != "read_write" {
                Some("Changes to a read-only sandbox root cannot be applied.".to_string())
            } else if file_root_violation {
                Some("A shared file root cannot create neighboring files.".to_string())
            } else {
                None
            };
            changes.push(FileChange {
                root_id: manifest_root.root_id.clone(),
                root_label: manifest_root.root_label.clone(),
                path: relative,
                kind: kind.to_string(),
                size,
                binary,
                preview,
                applicable: policy_error.is_none(),
                policy_error,
            });
        }
    }
    Ok(RunDiff {
        sandbox_id: sandbox_id.to_string(),
        source_root: baseline
            .roots
            .first()
            .map(|item| item.source_path.clone())
            .unwrap_or_default(),
        workspace_root: workspace.display().to_string(),
        roots: mappings,
        changes,
    })
}

pub fn apply_run_diff(app_data_dir: &Path, sandbox_id: &str) -> Result<ApplyResult, String> {
    let diff = run_diff(app_data_dir, sandbox_id)?;
    let root = sandbox_root(app_data_dir, sandbox_id)?;
    let mut baseline = load_run_manifest(&root)?;
    let backup = root.join(format!("apply-backup-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&backup).map_err(|error| error.to_string())?;
    let mut applied = Vec::new();
    let mut conflicts = Vec::new();
    let mut rejected = Vec::new();
    let mut rollback = Vec::<(PathBuf, Option<PathBuf>)>::new();

    for change in &diff.changes {
        let change_key = format!("{}:{}", change.root_id, change.path);
        if !change.applicable {
            rejected.push(change_key);
            continue;
        }
        let root_index = baseline
            .roots
            .iter()
            .position(|item| item.root_id == change.root_id)
            .ok_or_else(|| format!("sandbox manifest root is missing: {}", change.root_id))?;
        let manifest_root = &baseline.roots[root_index];
        let relative = relative_path(&change.path)?;
        let source_path = PathBuf::from(&manifest_root.source_path);
        let (source_root, target) = if manifest_root.kind == "file" {
            let canonical_source = source_path
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let parent = canonical_source
                .parent()
                .ok_or_else(|| "shared file source has no parent".to_string())?
                .to_path_buf();
            (parent, canonical_source)
        } else {
            let canonical_source = source_path
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let target = canonical_source.join(&relative);
            (canonical_source, target)
        };
        ensure_target_within_root(&source_root, &target)?;
        let current = if target.is_file() {
            Some(hash_file(&target)?)
        } else {
            None
        };
        if current != manifest_root.files.get(&change.path).cloned() {
            conflicts.push(change_key);
            continue;
        }
        if target.exists() {
            reject_unsafe_file(&target)?;
            let saved = backup.join(&change.root_id).join(&relative);
            if let Some(parent) = saved.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::copy(&target, &saved).map_err(|error| error.to_string())?;
            rollback.push((target.clone(), Some(saved)));
        } else {
            rollback.push((target.clone(), None));
        }
        let operation = (|| {
            if change.kind == "deleted" {
                if target.exists() {
                    fs::remove_file(&target).map_err(|error| error.to_string())?;
                }
            } else {
                let staged = PathBuf::from(&manifest_root.workspace_path).join(&relative);
                reject_unsafe_file(&staged)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let temporary =
                    target.with_file_name(format!(".lacowork-{}.tmp", uuid::Uuid::new_v4()));
                fs::copy(&staged, &temporary).map_err(|error| error.to_string())?;
                if let Err(error) = move_replace_file(&temporary, &target) {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = operation {
            for (restore_target, saved) in rollback.iter().rev() {
                let _ = if let Some(saved) = saved {
                    if restore_target.exists() {
                        let _ = fs::remove_file(restore_target);
                    }
                    fs::copy(saved, restore_target).map(|_| ())
                } else if restore_target.exists() {
                    fs::remove_file(restore_target)
                } else {
                    Ok(())
                };
            }
            return Err(format!("apply failed and was rolled back: {error}"));
        }
        applied.push(change_key);
    }
    if (!conflicts.is_empty() || !rejected.is_empty()) && !applied.is_empty() {
        let update_manifest = (|| -> Result<(), String> {
            for applied_path in &applied {
                let (root_id, path) = applied_path
                    .split_once(':')
                    .ok_or_else(|| "invalid applied change identifier".to_string())?;
                let manifest_root = baseline
                    .roots
                    .iter_mut()
                    .find(|item| item.root_id == root_id)
                    .ok_or_else(|| format!("sandbox manifest root is missing: {root_id}"))?;
                let workspace_manifest =
                    collect_manifest(Path::new(&manifest_root.workspace_path))?;
                if let Some(entry) = workspace_manifest.get(path) {
                    manifest_root.files.insert(path.to_string(), entry.clone());
                } else {
                    manifest_root.files.remove(path);
                }
            }
            write_file_atomically(
                &root.join("manifest.json"),
                &serde_json::to_vec_pretty(&baseline).map_err(|error| error.to_string())?,
            )
        })();
        if let Err(error) = update_manifest {
            for (restore_target, saved) in rollback.iter().rev() {
                let _ = if let Some(saved) = saved {
                    fs::copy(saved, restore_target).map(|_| ())
                } else if restore_target.exists() {
                    fs::remove_file(restore_target)
                } else {
                    Ok(())
                };
            }
            return Err(format!(
                "apply manifest update failed and file changes were rolled back: {error}"
            ));
        }
    }
    let _ = fs::remove_dir_all(backup);
    Ok(ApplyResult {
        applied,
        conflicts,
        rejected,
    })
}

fn write_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "atomic file target has no parent".to_string())?;
    let temporary = parent.join(format!(".lacowork-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if let Err(error) = move_replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn move_replace_file(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn move_replace_file(source: &Path, target: &Path) -> Result<(), String> {
    fs::rename(source, target).map_err(|error| error.to_string())
}

fn collect_manifest(root: &Path) -> Result<BTreeMap<String, ManifestEntry>, String> {
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut output = BTreeMap::new();
    collect_manifest_recursive(&canonical_root, &canonical_root, &mut output)?;
    Ok(output)
}

fn collect_manifest_recursive(
    root: &Path,
    directory: &Path,
    output: &mut BTreeMap<String, ManifestEntry>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() || is_windows_reparse_point(&path)? {
            return Err(format!("reparse point is not allowed: {}", path.display()));
        }
        if file_type.is_dir() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(should_ignore_dir)
            {
                continue;
            }
            collect_manifest_recursive(root, &path, output)?;
        } else if file_type.is_file() {
            reject_unsafe_file(&path)?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            output.insert(relative, hash_file(&path)?);
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<ManifestEntry, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(&bytes);
    let sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ManifestEntry {
        sha256,
        size: bytes.len() as u64,
    })
}

fn relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.contains(':') {
        return Err("invalid change path".to_string());
    }
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!("change path escapes workspace: {value}"));
    }
    Ok(path)
}

fn ensure_target_within_root(root: &Path, target: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "target has no parent".to_string())?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| "target parent escapes source root".to_string())?;
    }
    let canonical_parent = existing.canonicalize().map_err(|error| error.to_string())?;
    if !canonical_parent.starts_with(root) {
        return Err("apply target escapes source root".to_string());
    }
    Ok(())
}

fn reject_unsafe_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || metadata_is_windows_reparse_point(&metadata) {
        return Err(format!("reparse point is not allowed: {}", path.display()));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let file = fs::File::open(path).map_err(|error| error.to_string())?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if information.nNumberOfLinks > 1 {
            return Err(format!(
                "hard-linked file is not allowed: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn is_windows_reparse_point(path: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    Ok(metadata_is_windows_reparse_point(&metadata))
}

#[cfg(target_os = "windows")]
fn metadata_is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
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

        if file_type.is_symlink() || is_windows_reparse_point(&source_path)? {
            stats.skipped_files += 1;
            continue;
        }

        if file_type.is_dir() {
            let dir_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            if should_ignore_copy_dir(&dir_name) {
                stats.skipped_dirs.insert(dir_name);
                continue;
            }

            fs::create_dir_all(&target_path).map_err(|err| err.to_string())?;
            copy_dir_recursive(&source_path, &target_path, stats)?;
            continue;
        }

        if file_type.is_file() {
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

fn should_ignore_copy_dir(name: &str) -> bool {
    !name.eq_ignore_ascii_case(".git") && should_ignore_dir(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "localai-cowork-worker-sandbox-{}-{}",
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

    #[test]
    fn run_diff_and_apply_cover_create_modify_delete_and_conflicts() {
        let parent = test_root("changes");
        let app_data = parent.join("app-data");
        let source = parent.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("modify.txt"), "before").unwrap();
        fs::write(source.join("delete.txt"), "remove me").unwrap();

        let prepared = prepare_workspace_snapshot(&app_data, "run-changes", &source).unwrap();
        write_run_manifest(&app_data, "run-changes", &source).unwrap();
        let workspace = PathBuf::from(prepared.workspace_root);
        fs::write(workspace.join("modify.txt"), "after").unwrap();
        fs::remove_file(workspace.join("delete.txt")).unwrap();
        fs::write(workspace.join("created.bin"), [0u8, 1, 2, 3]).unwrap();

        let diff = run_diff(&app_data, "run-changes").unwrap();
        assert_eq!(diff.changes.len(), 3);
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "modify.txt" && change.kind == "modified"));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "delete.txt" && change.kind == "deleted"));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "created.bin"
                && change.kind == "created"
                && change.binary));

        // External edits after the manifest are conflicts and must never be overwritten.
        fs::write(source.join("modify.txt"), "external").unwrap();
        let applied = apply_run_diff(&app_data, "run-changes").unwrap();
        assert_eq!(applied.conflicts, vec!["workspace:modify.txt"]);
        assert_eq!(
            fs::read_to_string(source.join("modify.txt")).unwrap(),
            "external"
        );
        assert!(!source.join("delete.txt").exists());
        assert_eq!(
            fs::read(source.join("created.bin")).unwrap(),
            vec![0, 1, 2, 3]
        );

        // A retained copy after conflicts only presents the unresolved subset on retry.
        fs::write(source.join("modify.txt"), "before").unwrap();
        let retry_diff = run_diff(&app_data, "run-changes").unwrap();
        assert_eq!(
            retry_diff
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            vec!["modify.txt"]
        );
        let retried = apply_run_diff(&app_data, "run-changes").unwrap();
        assert!(retried.conflicts.is_empty());
        assert_eq!(
            fs::read_to_string(source.join("modify.txt")).unwrap(),
            "after"
        );

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn multi_root_manifest_groups_changes_and_never_applies_read_only_roots() {
        let parent = test_root("multi-root");
        let app_data = parent.join("app-data");
        let source_code = parent.join("source-code");
        let source_docs = parent.join("source-docs");
        fs::create_dir_all(&source_code).unwrap();
        fs::create_dir_all(&source_docs).unwrap();
        fs::write(source_code.join("main.txt"), "before").unwrap();
        fs::write(source_docs.join("guide.txt"), "original").unwrap();

        let prepared = prepare_workspace_snapshot_multi(
            &app_data,
            "run-multi-root",
            &[
                SnapshotRootInput {
                    root_id: "root-000".to_string(),
                    root_label: "Code".to_string(),
                    source_path: source_code.clone(),
                    kind: "folder".to_string(),
                    access: "read_write".to_string(),
                    is_primary: true,
                },
                SnapshotRootInput {
                    root_id: "root-001".to_string(),
                    root_label: "Docs".to_string(),
                    source_path: source_docs.clone(),
                    kind: "folder".to_string(),
                    access: "read_only".to_string(),
                    is_primary: false,
                },
            ],
        )
        .unwrap();
        let code_copy = PathBuf::from(&prepared.roots[0].workspace_path);
        let docs_copy = PathBuf::from(&prepared.roots[1].workspace_path);
        fs::write(code_copy.join("main.txt"), "after").unwrap();
        fs::write(docs_copy.join("guide.txt"), "blocked").unwrap();

        let diff = run_diff(&app_data, "run-multi-root").unwrap();
        assert_eq!(diff.changes.len(), 2);
        assert!(diff.changes.iter().any(|change| {
            change.root_id == "root-000" && change.root_label == "Code" && change.applicable
        }));
        assert!(diff.changes.iter().any(|change| {
            change.root_id == "root-001" && !change.applicable && change.policy_error.is_some()
        }));

        let result = apply_run_diff(&app_data, "run-multi-root").unwrap();
        assert_eq!(result.applied, vec!["root-000:main.txt"]);
        assert_eq!(result.rejected, vec!["root-001:guide.txt"]);
        assert_eq!(
            fs::read_to_string(source_code.join("main.txt")).unwrap(),
            "after"
        );
        assert_eq!(
            fs::read_to_string(source_docs.join("guide.txt")).unwrap(),
            "original"
        );

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn change_paths_reject_traversal_and_alternate_stream_syntax() {
        assert!(relative_path("../outside.txt").is_err());
        assert!(relative_path("folder/../../outside.txt").is_err());
        assert!(relative_path("safe.txt:stream").is_err());
        assert!(relative_path("C:/outside.txt").is_err());
    }

    #[test]
    fn git_metadata_is_available_in_the_copy_but_never_part_of_apply_diff() {
        let parent = test_root("git-copy");
        let app_data = parent.join("app-data");
        let source = parent.join("source");
        fs::create_dir_all(source.join(".git")).unwrap();
        fs::write(source.join(".git").join("config"), "[core]").unwrap();
        fs::write(source.join("tracked.txt"), "before").unwrap();

        let prepared = prepare_workspace_snapshot(&app_data, "run-git", &source).unwrap();
        write_run_manifest(&app_data, "run-git", &source).unwrap();
        let workspace = PathBuf::from(prepared.workspace_root);
        assert_eq!(
            fs::read_to_string(workspace.join(".git").join("config")).unwrap(),
            "[core]"
        );
        fs::write(workspace.join(".git").join("config"), "[changed]").unwrap();
        fs::write(workspace.join("tracked.txt"), "after").unwrap();

        let diff = run_diff(&app_data, "run-git").unwrap();
        assert_eq!(
            diff.changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tracked.txt"]
        );

        let _ = fs::remove_dir_all(parent);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn hard_linked_files_are_rejected() {
        let parent = test_root("hardlink");
        let first = parent.join("first.txt");
        let second = parent.join("second.txt");
        fs::write(&first, "shared").unwrap();
        fs::hard_link(&first, &second).unwrap();
        assert!(reject_unsafe_file(&first).is_err());
        assert!(reject_unsafe_file(&second).is_err());
        let _ = fs::remove_dir_all(parent);
    }
}
