//! Exports retained content, never live native paths or device/inode identities.
use super::*;
use cowork_domain::workspace_transfer::{self as transfer, CHUNK_BYTES};

impl FileBroker {
    pub fn workspace_transfer_manifest(
        &self,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<WorkspaceTransferManifest> {
        verify_workspace_snapshot(snapshot)?;
        workspaces::validate_entries(&snapshot.entries)?;
        self.portable_manifest_from_entries(&snapshot.entries)
    }

    pub(super) fn portable_manifest_from_entries(
        &self,
        entries: &[WorkspaceEntry],
    ) -> Result<WorkspaceTransferManifest> {
        workspaces::validate_entries(entries)?;
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for entry in entries {
            transfer::validate_path(&entry.path)?;
            let Some(sha256) = &entry.sha256 else {
                directories.push(entry.path.clone());
                continue;
            };
            // The complete source manifest was validated once above. Revalidating
            // its entire tree for every small file would make export quadratic.
            let bytes = read_regular_limit(&self.versions, sha256, transfer::MAX_FILE_BYTES)?
                .ok_or(Error::NotFound)?;
            if bytes.len() as u64 != entry.size || hash(&bytes) != *sha256 {
                return Err(Error::Validation(
                    "Retained content integrity check failed".into(),
                ));
            }
            let chunks = bytes
                .chunks(CHUNK_BYTES as usize)
                .enumerate()
                .map(|(index, chunk)| WorkspaceTransferChunk {
                    offset: index as u64 * CHUNK_BYTES,
                    size: chunk.len() as u64,
                    sha256: hash(chunk),
                })
                .collect();
            files.push(WorkspaceTransferFile {
                path: entry.path.clone(),
                size: entry.size,
                sha256: sha256.clone(),
                chunks,
            });
        }
        transfer::build(directories, files)
    }

    pub fn workspace_transfer_chunk(
        &self,
        snapshot: &WorkspaceSnapshot,
        input: &ReadWorkspaceTransferChunk,
    ) -> Result<BinaryFile> {
        transfer::validate_path(&input.path)?;
        if !input.offset.is_multiple_of(CHUNK_BYTES)
            || !transfer::valid_sha256(&input.expected_sha256)
        {
            return Err(Error::Validation("workspace_transfer_chunk_invalid".into()));
        }
        // The stored source snapshot determines the file. A caller-supplied digest
        // can never select an unrelated object from the content-addressed store.
        let file = self.download_snapshot(
            snapshot,
            &DownloadFile {
                path: input.path.clone(),
                expected_sha256: None,
            },
        )?;
        if input.offset >= file.metadata.size {
            return Err(Error::Validation(
                "workspace_transfer_chunk_out_of_bounds".into(),
            ));
        }
        let start = usize::try_from(input.offset).map_err(|_| Error::Forbidden)?;
        let end = file.bytes.len().min(start + CHUNK_BYTES as usize);
        let bytes = file.bytes[start..end].to_vec();
        let sha256 = hash(&bytes);
        if sha256 != input.expected_sha256 {
            return Err(Error::FileConflict);
        }
        Ok(BinaryFile {
            metadata: DownloadMetadata {
                file_name: format!("chunk-{}.bin", input.offset),
                size: bytes.len() as u64,
                sha256,
            },
            bytes,
        })
    }
}
