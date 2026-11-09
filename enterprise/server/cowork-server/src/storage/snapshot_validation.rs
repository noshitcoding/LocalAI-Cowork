//! Snapshot quotas, normalized paths, digest validation and secret warnings.

use crate::{error::ApiError, storage::envelope::KeyScope};
use chrono::{DateTime, Duration, Utc};
use cowork_contracts::SnapshotUploadFile;
use std::collections::{HashMap, HashSet};

pub const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;

const MAX_SNAPSHOT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

pub(super) fn validated_snapshot_expiry(
    scope: KeyScope,
    requested: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let now = Utc::now();
    match scope {
        KeyScope::DirectoryConnector(_) => Err(ApiError::Unprocessable(
            "directory key scopes cannot be used for content snapshots".to_owned(),
        )),
        KeyScope::User(_) => {
            let expires_at = requested.unwrap_or_else(|| now + Duration::days(30));
            if expires_at <= now || expires_at > now + Duration::days(30) {
                return Err(ApiError::Unprocessable(
                    "private snapshots must expire within 30 days".to_owned(),
                ));
            }
            Ok(Some(expires_at))
        }
        KeyScope::Team(_) => {
            if requested.is_some() {
                return Err(ApiError::Unprocessable(
                    "team snapshots are retained through project versions and cannot set expires_at"
                        .to_owned(),
                ));
            }
            Ok(None)
        }
    }
}

pub(super) fn validate_snapshot_files(
    total_bytes: u64,
    files: &[SnapshotUploadFile],
) -> Result<(), ApiError> {
    if total_bytes > MAX_SNAPSHOT_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "snapshot exceeds the supported {MAX_SNAPSHOT_BYTES}-byte limit"
        )));
    }
    let mut paths = HashSet::new();
    let mut computed_total = 0_u64;
    for file in files {
        validate_relative_path(&file.path)?;
        if !paths.insert(file.path.as_str()) {
            return Err(ApiError::Unprocessable(format!(
                "snapshot contains duplicate path {}",
                file.path
            )));
        }
        let chunk_total = file.chunks.iter().try_fold(0_u64, |sum, chunk| {
            validate_digest(&chunk.digest)?;
            if chunk.plaintext_size == 0 || chunk.plaintext_size > MAX_CHUNK_BYTES as u64 {
                return Err(ApiError::Unprocessable(format!(
                    "chunk {} has an invalid size",
                    chunk.digest
                )));
            }
            sum.checked_add(chunk.plaintext_size)
                .ok_or_else(|| ApiError::Unprocessable("snapshot byte count overflow".to_owned()))
        })?;
        if chunk_total != file.size {
            return Err(ApiError::Unprocessable(format!(
                "chunks for {} total {chunk_total} bytes, expected {}",
                file.path, file.size
            )));
        }
        computed_total = computed_total
            .checked_add(file.size)
            .ok_or_else(|| ApiError::Unprocessable("snapshot byte count overflow".to_owned()))?;
    }
    if computed_total != total_bytes {
        return Err(ApiError::Unprocessable(format!(
            "file sizes total {computed_total} bytes, expected {total_bytes}"
        )));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ApiError::Unprocessable(format!(
            "snapshot path is not a normalized relative path: {path}"
        )));
    }
    Ok(())
}

pub(super) fn validate_digest(digest: &str) -> Result<(), ApiError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::Unprocessable(
            "chunk digest must be lowercase SHA-256 hex".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn unique_digests(
    files: &[SnapshotUploadFile],
) -> Result<HashMap<String, u64>, ApiError> {
    let mut result = HashMap::new();
    for file in files {
        for chunk in &file.chunks {
            match result.insert(chunk.digest.clone(), chunk.plaintext_size) {
                Some(previous) if previous != chunk.plaintext_size => {
                    return Err(ApiError::Unprocessable(format!(
                        "chunk {} is declared with inconsistent sizes",
                        chunk.digest
                    )))
                }
                _ => {}
            }
        }
    }
    Ok(result)
}

pub(super) fn detect_secret_warnings(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let patterns = [
        ("-----BEGIN PRIVATE KEY", "possible_private_key"),
        ("-----BEGIN OPENSSH PRIVATE KEY", "possible_ssh_private_key"),
        ("AKIA", "possible_aws_access_key"),
        ("password=", "possible_password_assignment"),
        ("api_key", "possible_api_key_assignment"),
        ("client_secret", "possible_client_secret"),
    ];
    patterns
        .into_iter()
        .filter(|(needle, _)| text.contains(needle))
        .map(|(_, warning)| warning.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_and_windows_paths() {
        assert!(validate_relative_path("src/main.rs").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("C:\\secret").is_err());
    }
}
