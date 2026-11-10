//! Original once-only native/S3 staging, detached receipt capture on lost HTTP ACK.
use axum::body::Bytes;
use cowork_contracts::{DownloadMetadata, FileUpload, UploadState};
use cowork_domain::{Error, Result, hash};
use cowork_storage::{
    Store, TeamBlob, TeamBlobState, TeamBlobVersion, historical_imports::product::*,
};
use std::sync::Arc;
use uuid::Uuid;

pub(super) async fn observe(
    store: &Store,
    objects: Arc<crate::s3_store::S3Store>,
    broker: Option<Arc<cowork_broker::FileBroker>>,
    op: &HistoricalFileOperation,
    send: bool,
    bytes: Bytes,
) -> Result<()> {
    let result = physical(objects, broker, op, send, bytes).await;
    match result {
        Ok(payload) => {
            let receipt = serde_json::json!({"format":"m9-original-file-readback/1","preview_id":op.preview_id,"import_id":op.import_id,"owner":op.owner,"plan_sha256":op.plan_sha256,"source_id":op.source_id,"target_id":op.target_id,"project_id":op.project_id,"backend_id":op.backend_id,"original_request_sha256":op.request_sha256,"size":op.size,"sha256":op.sha256,"original_created_at":op.created_at,"readback_verified":true});
            let confirmed = HistoricalStagedFile::from_confirmed_adapter(
                op.preview_id,
                op.plan_sha256.clone(),
                op.source_id.clone(),
                op.target_id.clone(),
                op.project_id.clone(),
                op.size,
                op.sha256.clone(),
                payload,
                receipt,
            );
            if let Err(error) = store.receive_historical_file(op, confirmed).await {
                store.mark_historical_file_uncertain(op).await?;
                return Err(error);
            }
            Ok(())
        }
        Err(error) => {
            store.mark_historical_file_uncertain(op).await?;
            Err(error)
        }
    }
}
async fn physical(
    objects: Arc<crate::s3_store::S3Store>,
    broker: Option<Arc<cowork_broker::FileBroker>>,
    op: &HistoricalFileOperation,
    send: bool,
    bytes: Bytes,
) -> Result<serde_json::Value> {
    if bytes.len() as u64 != op.size || hash(&bytes) != op.sha256 {
        return Err(Error::Conflict);
    }
    match &op.owner {
        HistoricalOwner::PersonalNative { .. } => {
            if op.backend_id != "native-file-broker-v1" {
                return Err(Error::Conflict);
            }
            let upload = FileUpload {
                id: op.target_id.clone(),
                order_id: op.order_id.clone(),
                project_id: op.project_id.clone(),
                file: DownloadMetadata {
                    file_name: op.name.clone(),
                    size: op.size,
                    sha256: op.sha256.clone(),
                },
                state: UploadState::Ready,
                revision: 2,
                created_at: op.created_at,
            };
            let broker = broker.ok_or(Error::Forbidden)?;
            let descriptor = upload.clone();
            tokio::task::spawn_blocking(move || {
                if send {
                    broker.retain_upload(&descriptor, &bytes)?;
                }
                broker.verify_upload(&descriptor)
            })
            .await
            .map_err(|_| Error::UncertainEffect)??;
            serde_json::to_value(upload).map_err(|_| Error::Conflict)
        }
        HistoricalOwner::Organization {
            organization_id, ..
        } => {
            if op.backend_id != objects.backend_id() {
                return Err(Error::Conflict);
            }
            let id = Uuid::parse_str(&op.target_id).map_err(|_| Error::Conflict)?;
            let project = Uuid::parse_str(&op.project_id).map_err(|_| Error::Conflict)?;
            let version = TeamBlobVersion {
                blob_id: id,
                version: 1,
                size: op.size,
                sha256: op.sha256.clone(),
                object_key: format!("cowork/v1/{organization_id}/{project}/{id}/1/{}", op.sha256),
                backend_id: op.backend_id.clone(),
                state: TeamBlobState::Ready,
                created_at: op.created_at,
            };
            if send {
                objects.put(&version, &bytes).await?;
            }
            let original = objects.get(&version).await?;
            if original.len() as u64 != op.size || hash(&original) != op.sha256 {
                return Err(Error::Conflict);
            }
            let blob = TeamBlob {
                id,
                team_id: *organization_id,
                project_id: project,
                name: op.name.clone(),
                media_type: op.media_type.clone(),
                size: op.size,
                sha256: op.sha256.clone(),
                version: 1,
                revision: 2,
                state: TeamBlobState::Ready,
                created_at: op.created_at,
                updated_at: op.created_at,
            };
            Ok(serde_json::json!({"blob":blob,"version":version}))
        }
    }
}
