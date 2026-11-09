//! Freeze selected native capabilities locally. No remote transfer or execution.
use super::*;
use cowork_domain::execution_inputs::{self as inputs, CapturedExecutionInput};
use std::io::{Seek, SeekFrom};

pub struct PreparedInputCapture {
    pub capture: CapturedExecutionInput,
    // Keep source revocation and native writer fences until custody is committed.
    _locks: Vec<ResourceLock>,
}

impl FileBroker {
    pub fn prepare_execution_input(
        &self,
        order: &Order,
        request: &PrepareExecutionInput,
        snapshot_id: &str,
        at: i64,
    ) -> Result<PreparedInputCapture> {
        inputs::validate_request(request)?;
        if request.base_revision != order.revision {
            return Err(Error::Conflict);
        }
        let mut origins = Vec::new();
        let mut roots = Vec::new();
        let mut identities = std::collections::BTreeSet::new();
        for id in &request.folder_ids {
            let root = self.lookup_root(id, false)?;
            if root.grant.project_id != order.project_id
                || root.grant.order_id.as_ref().is_some_and(|v| v != &order.id)
                || !identities.insert(root.grant.resource_id.clone())
            {
                return Err(Error::Forbidden);
            }
            origins.push(root.grant.clone());
            roots.push(root);
        }
        // Stable native lock order; try_lock never waits while holding another.
        let mut lock_roots = roots.iter().collect::<Vec<_>>();
        lock_roots.sort_by(|a, b| a.lock_name.cmp(&b.lock_name));
        let mut locks = Vec::new();
        for root in lock_roots {
            locks.push(self.lock(root, false)?);
        }
        let mut entries = Vec::new();
        let mut size = 0;
        let mut sources = Vec::new();
        for (i, root) in roots.iter().enumerate() {
            let alias = inputs::source_alias(i);
            entries.push(WorkspaceEntry {
                path: alias.clone(),
                sha256: None,
                size: 0,
            });
            workspaces::capture_transfer_directory(
                &self.versions,
                &root.dir,
                &alias,
                1,
                &mut size,
                &mut entries,
            )?;
            sources.push(ExecutionInputSource {
                alias,
                name: root.grant.name.clone(),
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = self.portable_manifest_from_entries(&entries)?;
        let snapshot = ExecutionInputSnapshot {
            format_version: 1,
            id: snapshot_id.into(),
            order_id: order.id.clone(),
            project_id: order.project_id.clone(),
            order_revision: order.revision,
            request_sha256: inputs::request_hash(&order.id, request),
            created_at: at,
            sources,
            manifest,
        };
        let capture = CapturedExecutionInput {
            sha256: canonical_hash(&snapshot),
            snapshot,
            origins,
        };
        capture.validate_request(request)?;
        Ok(PreparedInputCapture {
            capture,
            _locks: locks,
        })
    }

    /// Private trusted-caller operation. A future controller must first prove the
    /// exact confirmed plan/recipient transfer grant; no HTTP route exposes this.
    pub fn execution_input_chunk(
        &self,
        capture: &CapturedExecutionInput,
        input: &ReadWorkspaceTransferChunk,
    ) -> Result<BinaryFile> {
        capture.validate()?;
        let alias = input.path.split('/').next().ok_or(Error::Forbidden)?;
        let index = capture
            .snapshot
            .sources
            .iter()
            .position(|s| s.alias == alias)
            .ok_or(Error::Forbidden)?;
        let origin = &capture.origins[index];
        let root = self.lookup_root(&origin.id, false)?;
        if &root.grant != origin {
            return Err(Error::Forbidden);
        }
        let _lock = self.lock(&root, false)?;
        let file = capture
            .snapshot
            .manifest
            .files
            .iter()
            .find(|f| f.path == input.path)
            .ok_or(Error::NotFound)?;
        let chunk = file
            .chunks
            .iter()
            .find(|c| c.offset == input.offset && c.sha256 == input.expected_sha256)
            .ok_or(Error::Forbidden)?;
        // Chunk hashes are already bound by the immutable custody manifest.
        // Read at most the requested 4 MiB, rather than rehashing an entire
        // 64-MiB blob for each chunk. Final whole-file verification belongs to
        // the receiver before publication/dispatch.
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut retained = self
            .versions
            .open_with(&file.sha256, &options)
            .map_err(storage_error)?;
        let before = retained.metadata().map_err(storage_error)?;
        if !before.is_file() || before.nlink() != 1 || before.len() != file.size {
            return Err(Error::FileConflict);
        }
        retained
            .seek(SeekFrom::Start(chunk.offset))
            .map_err(storage_error)?;
        let mut bytes = vec![0; chunk.size as usize];
        retained.read_exact(&mut bytes).map_err(storage_error)?;
        let after = retained.metadata().map_err(storage_error)?;
        if after.len() != before.len()
            || after.nlink() != 1
            || after.modified().ok() != before.modified().ok()
            || hash(&bytes) != chunk.sha256
        {
            return Err(Error::FileConflict);
        }
        Ok(BinaryFile {
            metadata: DownloadMetadata {
                file_name: format!("chunk-{}.bin", chunk.offset),
                size: chunk.size,
                sha256: chunk.sha256.clone(),
            },
            bytes,
        })
    }
}
