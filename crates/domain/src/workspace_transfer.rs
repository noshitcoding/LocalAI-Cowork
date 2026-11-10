//! Shared, versioned transfer validation. Native file identities stay at the broker.
use crate::{Error, Result, canonical_hash, hash};
use cowork_contracts::{WorkspaceTransferFile, WorkspaceTransferManifest};
use serde_json::json;
use std::collections::{BTreeSet, HashSet};

pub const CHUNK_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

fn invalid(reason: &str) -> Error {
    Error::Validation(reason.into())
}

pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Cross-platform aliases are refused before materialization. A receiver must
/// additionally create exclusively inside its private stage and reject all links.
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 1024 || path.split('/').count() > 65 {
        return Err(invalid("workspace_transfer_path_limit"));
    }
    for part in path.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.len() > 255
            || part.encode_utf16().count() > 255
            || part.ends_with(['.', ' '])
            || part.to_ascii_lowercase().starts_with(".cowork-")
            || part
                .chars()
                .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
        {
            return Err(invalid("workspace_transfer_path_invalid"));
        }
        let base = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let numbered = base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        if numbered
            || matches!(
                base.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
        {
            return Err(invalid("workspace_transfer_device_alias"));
        }
    }
    Ok(())
}

pub fn manifest_hash(manifest: &WorkspaceTransferManifest) -> String {
    canonical_hash(
        &json!({"format_version":manifest.format_version,"directories":manifest.directories,
        "files":manifest.files,"total_bytes":manifest.total_bytes}),
    )
}

pub fn validate(manifest: &WorkspaceTransferManifest) -> Result<()> {
    if manifest.format_version != 1
        || !valid_sha256(&manifest.sha256)
        || manifest.sha256 != manifest_hash(manifest)
        || manifest
            .directories
            .len()
            .checked_add(manifest.files.len())
            .is_none_or(|n| n > MAX_ENTRIES)
        || manifest.total_bytes > MAX_TOTAL_BYTES
        || serde_json::to_vec(manifest)
            .map_err(|_| invalid("workspace_transfer_encoding"))?
            .len()
            > MAX_MANIFEST_BYTES
    {
        return Err(invalid("workspace_transfer_manifest_invalid"));
    }
    let mut aliases = BTreeSet::new();
    let mut directories = HashSet::new();
    let mut previous = "";
    for directory in &manifest.directories {
        validate_path(directory)?;
        if directory.as_str() <= previous
            || !aliases.insert(directory.to_lowercase().to_uppercase())
        {
            return Err(invalid("workspace_transfer_path_collision"));
        }
        parent_present(directory, &directories)?;
        directories.insert(directory.as_str());
        previous = directory;
    }
    previous = "";
    let mut total = 0u64;
    for file in &manifest.files {
        validate_path(&file.path)?;
        if file.path.as_str() <= previous
            || !aliases.insert(file.path.to_lowercase().to_uppercase())
        {
            return Err(invalid("workspace_transfer_path_collision"));
        }
        parent_present(&file.path, &directories)?;
        if file.size > MAX_FILE_BYTES
            || !valid_sha256(&file.sha256)
            || file.size == 0 && file.sha256 != hash(b"")
            || file.chunks.len() as u64 != file.size.div_ceil(CHUNK_BYTES)
        {
            return Err(invalid("workspace_transfer_file_invalid"));
        }
        let mut offset = 0u64;
        for chunk in &file.chunks {
            if chunk.offset != offset
                || chunk.size != CHUNK_BYTES.min(file.size - offset)
                || !valid_sha256(&chunk.sha256)
            {
                return Err(invalid("workspace_transfer_chunk_invalid"));
            }
            offset = offset
                .checked_add(chunk.size)
                .ok_or_else(|| invalid("workspace_transfer_overflow"))?;
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| invalid("workspace_transfer_overflow"))?;
        if total > MAX_TOTAL_BYTES {
            return Err(invalid("workspace_transfer_byte_limit"));
        }
        previous = &file.path;
    }
    if total != manifest.total_bytes {
        return Err(invalid("workspace_transfer_total_mismatch"));
    }
    Ok(())
}

fn parent_present(path: &str, directories: &HashSet<&str>) -> Result<()> {
    if let Some((parent, _)) = path.rsplit_once('/')
        && !directories.contains(parent)
    {
        return Err(invalid("workspace_transfer_parent_missing"));
    }
    Ok(())
}

pub fn build(
    directories: Vec<String>,
    files: Vec<WorkspaceTransferFile>,
) -> Result<WorkspaceTransferManifest> {
    let total_bytes = files
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| invalid("workspace_transfer_overflow"))?;
    let mut manifest = WorkspaceTransferManifest {
        format_version: 1,
        directories,
        files,
        total_bytes,
        sha256: String::new(),
    };
    manifest.sha256 = manifest_hash(&manifest);
    validate(&manifest)?;
    Ok(manifest)
}
