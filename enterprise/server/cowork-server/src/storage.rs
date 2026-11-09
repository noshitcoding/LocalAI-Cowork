//! Encrypted object storage, snapshot lifecycle, artifacts and project history.

mod artifacts;
pub(crate) mod content_federation;
mod envelope;
mod executor_access;
mod merges;
mod object_store;
mod retention;
mod snapshot_repository;
mod snapshot_validation;
mod snapshots;
mod versions;

pub use artifacts::{
    download_run_artifact, list_run_artifacts, persist_run_artifact, upload_executor_run_artifact,
    upload_run_attachment, MAX_AGENT_ARTIFACT_BYTES,
};
pub(crate) use envelope::SealedValue;
pub use envelope::SecretStore;
pub use merges::{apply_project_merge, review_project_merge};
pub use object_store::ObjectStore;
pub use retention::garbage_collect;
pub use snapshot_validation::MAX_CHUNK_BYTES;
pub use snapshots::{
    begin_executor_run_result_snapshot, begin_snapshot_upload, commit_executor_run_result_snapshot,
    commit_snapshot, delete_snapshot, download_executor_run_chunk, download_snapshot_chunk,
    get_executor_run_result_snapshot, get_executor_run_snapshot, get_snapshot,
    snapshot_upload_status, upload_executor_run_result_chunk, upload_snapshot_chunk,
};
pub use versions::{apply_project_version, create_project_version, list_project_versions};
