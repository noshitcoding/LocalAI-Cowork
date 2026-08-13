use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::{Component, Path as FsPath},
    sync::Arc,
    time::Duration as StdDuration,
};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use axum::{
    body::{Body, Bytes},
    extract::{Extension, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use cowork_contracts::{
    ApplyProjectMergeRequest, ApplyProjectVersionRequest, BeginSnapshotUploadRequest,
    ExecutorTarget, MergeFileReview, MergeFileStatus, MergeResolutionChoice, ProjectMergeReview,
    ProjectRole, ProjectVersion, RunArtifact, RunEventKind, SnapshotChunk, SnapshotChunkReceipt,
    SnapshotFile, SnapshotManifest, SnapshotUploadChunk, SnapshotUploadFile, SnapshotUploadSession,
    SCHEMA_VERSION,
};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::{ExecutorPrincipal, Principal},
    config::ObjectStoreConfig,
    db,
    error::ApiError,
    governance, organization, AppState,
};

type HmacSha256 = Hmac<Sha256>;

pub const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_AGENT_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_TEXT_MERGE_BYTES: u64 = 5 * 1024 * 1024;
const AWS_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone)]
pub struct ObjectStore {
    s3: S3Client,
    master_key: [u8; 32],
}

#[derive(Clone)]
struct S3Client {
    http: Client,
    endpoint: Url,
    region: String,
    bucket: String,
    addressing_style: S3AddressingStyle,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum S3AddressingStyle {
    Path,
    VirtualHosted,
}

#[derive(Clone, Copy)]
enum KeyScope {
    User(Uuid),
    Team(Uuid),
}

#[derive(Debug, Deserialize)]
pub struct MergeReviewQuery {
    base_version_id: Uuid,
    current_version_id: Uuid,
    result_version_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ExecutorArtifactQuery {
    path: String,
    source: String,
    source_event_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UserAttachmentQuery {
    name: String,
}

#[derive(Clone)]
struct VersionFiles {
    version: ProjectVersion,
    manifest_id: Uuid,
    files: HashMap<String, SnapshotFile>,
}

struct FileAnalysis {
    review: MergeFileReview,
    auto_merged: Option<Vec<u8>>,
}

impl KeyScope {
    fn kind(self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Team(_) => "team",
        }
    }

    fn id(self) -> Uuid {
        match self {
            Self::User(id) | Self::Team(id) => id,
        }
    }

    fn key_id(self) -> String {
        format!("master-v1:{}:{}", self.kind(), self.id())
    }
}

struct EncryptedChunk {
    ciphertext: Vec<u8>,
    wrapped_data_key: Vec<u8>,
    nonce: [u8; 12],
    wrap_nonce: [u8; 12],
}

#[derive(Debug)]
pub(crate) struct SealedValue {
    pub ciphertext: Vec<u8>,
    pub encrypted_data_key: Vec<u8>,
    pub nonce: [u8; 12],
    pub wrap_nonce: [u8; 12],
}

impl ObjectStore {
    pub fn from_config(config: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let master_key = decode_master_key(&config.master_key)?;
        let endpoint = Url::parse(config.endpoint.trim())?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https"),
            "S3 endpoint must use HTTP or HTTPS"
        );
        anyhow::ensure!(
            endpoint.query().is_none() && endpoint.fragment().is_none(),
            "S3 endpoint cannot contain query or fragment"
        );
        anyhow::ensure!(
            endpoint.username().is_empty() && endpoint.password().is_none(),
            "S3 endpoint cannot contain credentials"
        );
        anyhow::ensure!(
            matches!(endpoint.path(), "" | "/"),
            "S3 endpoint cannot contain a path prefix"
        );
        anyhow::ensure!(
            endpoint.host_str().is_some(),
            "S3 endpoint must contain a host"
        );
        anyhow::ensure!(
            !config.bucket.trim().is_empty()
                && !config.bucket.contains('/')
                && !config.bucket.contains('\\')
                && !config.bucket.chars().any(char::is_control),
            "S3 bucket name is invalid"
        );
        let addressing_style = match config.addressing_style.trim() {
            "path" => S3AddressingStyle::Path,
            "virtual_hosted" | "virtual-hosted" => S3AddressingStyle::VirtualHosted,
            other => anyhow::bail!(
                "COWORK_S3_ADDRESSING_STYLE must be path or virtual_hosted; got {other}"
            ),
        };
        if addressing_style == S3AddressingStyle::VirtualHosted {
            let host = endpoint.host_str().unwrap_or_default();
            anyhow::ensure!(
                host.parse::<IpAddr>().is_err(),
                "virtual-hosted S3 addressing requires a DNS endpoint"
            );
            anyhow::ensure!(
                valid_virtual_host_bucket(&config.bucket),
                "virtual-hosted S3 addressing requires a DNS-compatible bucket name"
            );
        }
        Ok(Self {
            s3: S3Client {
                http: Client::builder()
                    .connect_timeout(StdDuration::from_secs(10))
                    .timeout(StdDuration::from_secs(5 * 60))
                    .build()?,
                endpoint,
                region: config.region.clone(),
                bucket: config.bucket.clone(),
                addressing_style,
                access_key: config.access_key.clone(),
                secret_key: config.secret_key.clone(),
                session_token: config.session_token.clone(),
            },
            master_key,
        })
    }

    async fn put_encrypted(
        &self,
        scope: KeyScope,
        digest_hex: &str,
        plaintext: &[u8],
    ) -> Result<(String, EncryptedChunk), ApiError> {
        let encrypted = self.encrypt(scope, plaintext)?;
        let object_key = format!(
            "chunks/{}/{}/{}/{}",
            scope.kind(),
            scope.id(),
            &digest_hex[..2],
            digest_hex
        );
        self.s3
            .request(Method::PUT, &object_key, Some(encrypted.ciphertext.clone()))
            .await?;
        Ok((object_key, encrypted))
    }

    async fn get_decrypted(
        &self,
        scope: KeyScope,
        object_key: &str,
        wrapped_data_key: &[u8],
        nonce: &[u8],
        wrap_nonce: &[u8],
    ) -> Result<Vec<u8>, ApiError> {
        let ciphertext = self.s3.request(Method::GET, object_key, None).await?;
        self.decrypt(scope, &ciphertext, wrapped_data_key, nonce, wrap_nonce)
    }

    async fn delete(&self, object_key: &str) -> Result<(), ApiError> {
        self.s3.request(Method::DELETE, object_key, None).await?;
        Ok(())
    }

    pub(crate) fn seal_for_user(
        &self,
        user_id: Uuid,
        plaintext: &[u8],
    ) -> Result<SealedValue, ApiError> {
        let encrypted = self.encrypt(KeyScope::User(user_id), plaintext)?;
        Ok(SealedValue {
            ciphertext: encrypted.ciphertext,
            encrypted_data_key: encrypted.wrapped_data_key,
            nonce: encrypted.nonce,
            wrap_nonce: encrypted.wrap_nonce,
        })
    }

    pub(crate) fn open_for_user(
        &self,
        user_id: Uuid,
        sealed: &SealedValue,
    ) -> Result<Vec<u8>, ApiError> {
        self.decrypt(
            KeyScope::User(user_id),
            &sealed.ciphertext,
            &sealed.encrypted_data_key,
            &sealed.nonce,
            &sealed.wrap_nonce,
        )
    }

    pub(crate) fn seal_for_team(
        &self,
        team_id: Uuid,
        plaintext: &[u8],
    ) -> Result<SealedValue, ApiError> {
        let encrypted = self.encrypt(KeyScope::Team(team_id), plaintext)?;
        Ok(SealedValue {
            ciphertext: encrypted.ciphertext,
            encrypted_data_key: encrypted.wrapped_data_key,
            nonce: encrypted.nonce,
            wrap_nonce: encrypted.wrap_nonce,
        })
    }

    pub(crate) fn open_for_team(
        &self,
        team_id: Uuid,
        sealed: &SealedValue,
    ) -> Result<Vec<u8>, ApiError> {
        self.decrypt(
            KeyScope::Team(team_id),
            &sealed.ciphertext,
            &sealed.encrypted_data_key,
            &sealed.nonce,
            &sealed.wrap_nonce,
        )
    }

    fn encrypt(&self, scope: KeyScope, plaintext: &[u8]) -> Result<EncryptedChunk, ApiError> {
        let mut data_key = [0_u8; 32];
        let mut nonce = [0_u8; 12];
        let mut wrap_nonce = [0_u8; 12];
        getrandom::fill(&mut data_key).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("random key generation failed: {error}"))
        })?;
        getrandom::fill(&mut nonce).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("random nonce generation failed: {error}"))
        })?;
        getrandom::fill(&mut wrap_nonce).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!(
                "random wrap nonce generation failed: {error}"
            ))
        })?;
        let cipher = Aes256Gcm::new_from_slice(&data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid generated data key")))?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("chunk encryption failed")))?;
        let scope_key = self.scope_key(scope)?;
        let wrapper = Aes256Gcm::new_from_slice(&scope_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid scope key")))?;
        let wrapped_data_key = wrapper
            .encrypt(Nonce::from_slice(&wrap_nonce), data_key.as_slice())
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("data-key wrapping failed")))?;
        data_key.fill(0);
        Ok(EncryptedChunk {
            ciphertext,
            wrapped_data_key,
            nonce,
            wrap_nonce,
        })
    }

    fn decrypt(
        &self,
        scope: KeyScope,
        ciphertext: &[u8],
        wrapped_data_key: &[u8],
        nonce: &[u8],
        wrap_nonce: &[u8],
    ) -> Result<Vec<u8>, ApiError> {
        if nonce.len() != 12 || wrap_nonce.len() != 12 {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "invalid encrypted chunk nonce"
            )));
        }
        let scope_key = self.scope_key(scope)?;
        let wrapper = Aes256Gcm::new_from_slice(&scope_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid scope key")))?;
        let mut data_key = wrapper
            .decrypt(Nonce::from_slice(wrap_nonce), wrapped_data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("data-key unwrap failed")))?;
        let cipher = Aes256Gcm::new_from_slice(&data_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid unwrapped data key")))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("chunk authentication failed")))?;
        data_key.fill(0);
        Ok(plaintext)
    }

    fn scope_key(&self, scope: KeyScope) -> Result<[u8; 32], ApiError> {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.master_key)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid storage master key")))?;
        mac.update(b"open-cowork-envelope-v1\0");
        mac.update(scope.kind().as_bytes());
        mac.update(b"\0");
        mac.update(scope.id().as_bytes());
        Ok(mac.finalize().into_bytes().into())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn persist_run_artifact(
    pool: &PgPool,
    store: &ObjectStore,
    run_id: Uuid,
    project_id: Uuid,
    creator_user_id: Uuid,
    workspace_path: &str,
    source: &str,
    source_event_id: Option<Uuid>,
    plaintext: &[u8],
) -> Result<serde_json::Value, ApiError> {
    let scope = project_scope(pool, project_id, creator_user_id).await?;
    let artifact_id = Uuid::new_v4();
    let name = workspace_path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unprocessable("artifact path has no file name".to_owned()))?;
    let digest = Sha256::digest(plaintext).to_vec();
    if let Some(source_event_id) = source_event_id {
        if let Some(row) =
            sqlx::query("SELECT * FROM run_artifacts WHERE run_id = $1 AND source_event_id = $2")
                .bind(run_id)
                .bind(source_event_id)
                .fetch_optional(pool)
                .await?
        {
            let metadata: Value = row.try_get("metadata")?;
            if row.try_get::<Vec<u8>, _>("digest")? != digest
                || row.try_get::<i64, _>("size_bytes")? != plaintext.len() as i64
                || metadata.get("workspace_path").and_then(Value::as_str) != Some(workspace_path)
                || metadata.get("source").and_then(Value::as_str) != Some(source)
            {
                return Err(ApiError::Conflict(
                    "source_event_id was already used for different artifact content".to_owned(),
                ));
            }
            return Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "id": row.try_get::<Uuid, _>("id")?,
                "run_id": run_id,
                "revision": row.try_get::<i64, _>("revision")?,
                "kind": row.try_get::<String, _>("kind")?,
                "media_type": row.try_get::<String, _>("media_type")?,
                "name": row.try_get::<String, _>("name")?,
                "digest": hex::encode(&digest),
                "size_bytes": plaintext.len(),
                "workspace_path": workspace_path,
                "storage": "object_store_encrypted",
            }));
        }
    }
    let encrypted = store.encrypt(scope, plaintext)?;
    let object_key = format!(
        "artifacts/{}/{}/{}/{}",
        scope.kind(),
        scope.id(),
        run_id,
        artifact_id
    );
    store
        .s3
        .request(Method::PUT, &object_key, Some(encrypted.ciphertext.clone()))
        .await?;
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM run_artifacts WHERE run_id = $1 AND name = $2",
    )
    .bind(run_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    let (kind, media_type) = artifact_media(name);
    let inserted = sqlx::query(
        "INSERT INTO run_artifacts (id, run_id, revision, kind, media_type, name, object_key, digest, size_bytes, metadata, key_scope_type, key_scope_id, encrypted_data_key, nonce, wrap_nonce, source_event_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(artifact_id)
    .bind(run_id)
    .bind(revision)
    .bind(kind)
    .bind(media_type)
    .bind(name)
    .bind(&object_key)
    .bind(&digest)
    .bind(plaintext.len() as i64)
    .bind(json!({"workspace_path": workspace_path, "source": source}))
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .bind(source_event_id)
    .execute(pool)
    .await;
    if let Err(error) = inserted {
        let _ = store.delete(&object_key).await;
        return Err(error.into());
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "id": artifact_id,
        "run_id": run_id,
        "revision": revision,
        "kind": kind,
        "media_type": media_type,
        "name": name,
        "digest": hex::encode(digest),
        "size_bytes": plaintext.len(),
        "workspace_path": workspace_path,
        "storage": "object_store_encrypted",
    }))
}

fn artifact_media(name: &str) -> (&'static str, &'static str) {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "webp" => ("image", "image/webp"),
        "pdf" => ("document", "application/pdf"),
        "zip" => ("trace", "application/zip"),
        "webm" => ("video", "video/webm"),
        "json" => ("log", "application/json"),
        "txt" | "log" => ("log", "text/plain; charset=utf-8"),
        _ => ("file", "application/octet-stream"),
    }
}

pub async fn list_run_artifacts(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<RunArtifact>>, ApiError> {
    let run = sqlx::query("SELECT project_id, thread_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.try_get("project_id")?,
        run.try_get("thread_id")?,
        ProjectRole::Viewer,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM run_artifacts WHERE run_id = $1 AND deleted_at IS NULL ORDER BY created_at, id",
    )
    .bind(run_id)
    .fetch_all(&state.pool)
    .await?;
    let artifacts = rows
        .iter()
        .map(row_to_artifact)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(artifacts))
}

pub async fn download_run_artifact(
    State(state): State<AppState>,
    Path((run_id, artifact_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let row = sqlx::query(
        "SELECT artifact.*, run.project_id, run.thread_id FROM run_artifacts artifact JOIN runs run ON run.id = artifact.run_id WHERE artifact.id = $1 AND artifact.run_id = $2 AND artifact.deleted_at IS NULL",
    )
    .bind(artifact_id)
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("artifact {artifact_id} was not found")))?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        row.try_get("project_id")?,
        row.try_get("thread_id")?,
        ProjectRole::Viewer,
    )
    .await?;
    let store = state
        .object_store
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("object storage is not configured".to_owned()))?;
    let scope = row_scope(&row)?;
    let wrapped_data_key = row
        .try_get::<Option<Vec<u8>>, _>("encrypted_data_key")?
        .ok_or_else(|| ApiError::Conflict("artifact has no encrypted data key".to_owned()))?;
    let nonce = row
        .try_get::<Option<Vec<u8>>, _>("nonce")?
        .ok_or_else(|| ApiError::Conflict("artifact has no encryption nonce".to_owned()))?;
    let wrap_nonce = row
        .try_get::<Option<Vec<u8>>, _>("wrap_nonce")?
        .ok_or_else(|| ApiError::Conflict("artifact has no wrapped-key nonce".to_owned()))?;
    let object_key: String = row.try_get("object_key")?;
    let bytes = store
        .get_decrypted(scope, &object_key, &wrapped_data_key, &nonce, &wrap_nonce)
        .await?;
    let expected_digest: Vec<u8> = row.try_get("digest")?;
    if Sha256::digest(&bytes).as_slice() != expected_digest.as_slice() {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "artifact digest verification failed"
        )));
    }
    let name: String = row.try_get("name")?;
    let media_type: String = row.try_get("media_type")?;
    let disposition_name = utf8_percent_encode(&name, AWS_ENCODE_SET).to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, media_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename*=UTF-8''{disposition_name}"),
        )
        .body(Body::from(bytes))
        .map_err(|error| ApiError::Internal(error.into()))
}

pub async fn upload_run_attachment(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<UserAttachmentQuery>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_attachment_name(&query.name)?;
    if body.is_empty() {
        return Err(ApiError::Unprocessable(
            "attachment must not be empty".to_owned(),
        ));
    }
    let project_id = sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    let store = require_store(&state)?;
    let workspace_path = format!("artifacts/user-uploads/{}", query.name);
    let mut payload = persist_run_artifact(
        &state.pool,
        &store,
        run_id,
        project_id,
        principal.user_id,
        &workspace_path,
        "user_attachment",
        None,
        &body,
    )
    .await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("uploaded_by_user_id".to_owned(), json!(principal.user_id));
    }
    let mut tx = state.pool.begin().await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::ArtifactCreated,
        payload.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(payload)))
}

fn row_to_artifact(row: &sqlx::postgres::PgRow) -> Result<RunArtifact, ApiError> {
    let digest: Vec<u8> = row.try_get("digest")?;
    Ok(RunArtifact {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        revision: row.try_get("revision")?,
        kind: row.try_get("kind")?,
        media_type: row.try_get("media_type")?,
        name: row.try_get("name")?,
        digest: hex::encode(digest),
        size_bytes: row.try_get::<i64, _>("size_bytes")? as u64,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

impl S3Client {
    fn request_location(&self, object_key: &str) -> Result<(Url, String, String), ApiError> {
        let encoded_key = object_key
            .split('/')
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        let canonical_uri = match self.addressing_style {
            S3AddressingStyle::Path if encoded_key.is_empty() => {
                format!("/{}", encode_segment(&self.bucket))
            }
            S3AddressingStyle::Path => {
                format!("/{}/{}", encode_segment(&self.bucket), encoded_key)
            }
            S3AddressingStyle::VirtualHosted if encoded_key.is_empty() => "/".to_owned(),
            S3AddressingStyle::VirtualHosted => format!("/{encoded_key}"),
        };
        let mut url = self.endpoint.clone();
        if self.addressing_style == S3AddressingStyle::VirtualHosted {
            let host = format!(
                "{}.{}",
                self.bucket,
                self.endpoint.host_str().unwrap_or_default()
            );
            url.set_host(Some(&host)).map_err(|_| {
                ApiError::Internal(anyhow::anyhow!("invalid virtual-hosted S3 endpoint"))
            })?;
        }
        url.set_path(&canonical_uri);
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_owned(),
        };
        Ok((url, canonical_uri, host))
    }

    async fn request(
        &self,
        method: Method,
        object_key: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ApiError> {
        let (parsed, canonical_uri, host) = self.request_location(object_key)?;
        let payload = body.as_deref().unwrap_or_default();
        let payload_hash = hex::encode(Sha256::digest(payload));
        let now = Utc::now();
        let amz_date = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        let short_date = format!("{:04}{:02}{:02}", now.year(), now.month(), now.day());
        let (canonical_headers, signed_headers) = match &self.session_token {
            Some(token) => (
                format!(
                    "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\nx-amz-security-token:{}\n",
                    token.trim()
                ),
                "host;x-amz-content-sha256;x-amz-date;x-amz-security-token",
            ),
            None => (
                format!(
                    "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
                ),
                "host;x-amz-content-sha256;x-amz-date",
            ),
        };
        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri,
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let credential_scope = format!("{short_date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let signing_key = aws_signing_key(&self.secret_key, &short_date, &self.region)?;
        let signature = hex::encode(hmac_bytes(&signing_key, string_to_sign.as_bytes())?);
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );
        let mut request = self
            .http
            .request(method, parsed)
            .header("host", host)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("authorization", authorization);
        if let Some(token) = &self.session_token {
            request = request.header("x-amz-security-token", token.trim());
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("object-store request failed: {error}"))
        })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("object-store response failed: {error}"))
        })?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes);
            return Err(ApiError::Internal(anyhow::anyhow!(
                "object store returned {status}: {}",
                detail.chars().take(1000).collect::<String>()
            )));
        }
        Ok(bytes.to_vec())
    }
}

pub async fn begin_snapshot_upload(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<BeginSnapshotUploadRequest>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    begin_snapshot_upload_for(&state, principal.user_id, request, None).await
}

async fn begin_snapshot_upload_for(
    state: &AppState,
    user_id: Uuid,
    request: BeginSnapshotUploadRequest,
    source_run_id: Option<Uuid>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    require_store(state)?;
    organization::ensure_project_role(
        &state.pool,
        user_id,
        request.project_id,
        ProjectRole::Runner,
    )
    .await?;
    validate_snapshot_files(request.total_bytes, &request.files)?;
    let scope = project_scope(&state.pool, request.project_id, user_id).await?;
    let expires_at = validated_snapshot_expiry(scope, request.expires_at)?;
    let unique_digests = unique_digests(&request.files)?;
    if let Some(source_run_id) = source_run_id {
        if let Some(row) = sqlx::query("SELECT * FROM snapshot_manifests WHERE source_run_id = $1")
            .bind(source_run_id)
            .fetch_optional(&state.pool)
            .await?
        {
            if row.try_get::<Uuid, _>("project_id")? != request.project_id
                || row.try_get::<Uuid, _>("created_by")? != user_id
            {
                return Err(ApiError::Conflict(
                    "the run result snapshot belongs to a different project or user".to_owned(),
                ));
            }
            let existing_total = u64::try_from(row.try_get::<i64, _>("total_bytes")?)
                .map_err(|error| ApiError::Internal(error.into()))?;
            let existing_manifest: Value = row.try_get("manifest")?;
            if existing_total != request.total_bytes
                || existing_manifest != serde_json::to_value(&request.files)?
            {
                return Err(ApiError::Conflict(
                    "the retried result snapshot manifest differs from its original declaration"
                        .to_owned(),
                ));
            }
            return Ok((
                StatusCode::OK,
                Json(snapshot_upload_session_from_row(state, &row).await?),
            ));
        }
    }
    let manifest_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    governance::enforce_storage_quota_tx(&mut tx, scope.kind(), scope.id(), request.total_bytes)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO snapshot_manifests (
            id, project_id, created_by, key_scope_type, key_scope_id,
            encryption_key_id, total_bytes, file_count, manifest, status,
            expires_at, source_run_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'uploading', $10, $11)
        "#,
    )
    .bind(manifest_id)
    .bind(request.project_id)
    .bind(user_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(scope.key_id())
    .bind(
        i64::try_from(request.total_bytes)
            .map_err(|_| ApiError::Unprocessable("snapshot total_bytes is too large".to_owned()))?,
    )
    .bind(
        i64::try_from(request.files.len())
            .map_err(|_| ApiError::Unprocessable("snapshot contains too many files".to_owned()))?,
    )
    .bind(serde_json::to_value(&request.files)?)
    .bind(expires_at)
    .bind(source_run_id)
    .execute(&mut *tx)
    .await?;
    let reserved =
        reserve_existing_chunks(&mut tx, manifest_id, scope, unique_digests.keys()).await?;
    tx.commit().await?;
    let missing_chunks = unique_digests
        .keys()
        .filter(|digest| !reserved.contains(*digest))
        .cloned()
        .collect();
    Ok((
        StatusCode::CREATED,
        Json(SnapshotUploadSession {
            schema_version: SCHEMA_VERSION,
            manifest_id,
            missing_chunks,
            max_chunk_bytes: MAX_CHUNK_BYTES as u64,
            expires_at,
            warnings: Vec::new(),
        }),
    ))
}

pub async fn snapshot_upload_status(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotUploadSession>, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Runner,
    )
    .await?;
    Ok(Json(snapshot_upload_session_from_row(&state, &row).await?))
}

async fn snapshot_upload_session_from_row(
    state: &AppState,
    row: &sqlx::postgres::PgRow,
) -> Result<SnapshotUploadSession, ApiError> {
    let files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let scope = row_scope(row)?;
    let unique = unique_digests(&files)?;
    let existing = existing_digests(&state.pool, scope, &unique).await?;
    let missing_chunks = unique
        .keys()
        .filter(|digest| !existing.contains(*digest))
        .cloned()
        .collect();
    Ok(SnapshotUploadSession {
        schema_version: SCHEMA_VERSION,
        manifest_id: row.try_get("id")?,
        missing_chunks,
        max_chunk_bytes: MAX_CHUNK_BYTES as u64,
        expires_at: row.try_get("expires_at")?,
        warnings: serde_json::from_value(row.try_get("warnings")?)?,
    })
}

pub async fn upload_snapshot_chunk(
    State(state): State<AppState>,
    Path((manifest_id, digest_hex)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Result<(StatusCode, Json<SnapshotChunkReceipt>), ApiError> {
    let store = require_store(&state)?;
    validate_digest(&digest_hex)?;
    if body.len() > MAX_CHUNK_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "chunk exceeds maximum of {MAX_CHUNK_BYTES} bytes"
        )));
    }
    let manifest = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Runner,
    )
    .await?;
    let status: String = manifest.try_get("status")?;
    if status != "uploading" {
        return Err(ApiError::Conflict(
            "chunks can only be uploaded while the snapshot is uploading".to_owned(),
        ));
    }
    let files: Vec<SnapshotUploadFile> = serde_json::from_value(manifest.try_get("manifest")?)?;
    let expected = unique_digests(&files)?
        .get(&digest_hex)
        .copied()
        .ok_or_else(|| ApiError::Unprocessable("chunk is not part of this manifest".to_owned()))?;
    if expected != body.len() as u64 {
        return Err(ApiError::Unprocessable(format!(
            "chunk size is {}, expected {expected}",
            body.len()
        )));
    }
    let actual_digest = hex::encode(Sha256::digest(&body));
    if actual_digest != digest_hex {
        return Err(ApiError::Unprocessable(
            "chunk SHA-256 digest does not match its URL".to_owned(),
        ));
    }
    let scope = row_scope(&manifest)?;
    let digest = hex::decode(&digest_hex)
        .map_err(|_| ApiError::Unprocessable("invalid chunk digest".to_owned()))?;
    let mut tx = state.pool.begin().await?;
    // Serialize concurrent uploads of the same scope/digest without relying on
    // a process-local mutex (API replicas remain safe).
    let lock_name = format!("{}:{}:{digest_hex}", scope.kind(), scope.id());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query(
        r#"
        SELECT plaintext_size, ciphertext_size FROM snapshot_chunks
        WHERE key_scope_type = $1 AND key_scope_id = $2
          AND plaintext_digest = $3 AND status = 'ready'
        "#,
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .fetch_optional(&mut *tx)
    .await?
    {
        insert_chunk_reservation(&mut tx, manifest_id, scope, &digest).await?;
        tx.commit().await?;
        return Ok((
            StatusCode::OK,
            Json(SnapshotChunkReceipt {
                schema_version: SCHEMA_VERSION,
                manifest_id,
                digest: digest_hex,
                plaintext_size: u64::try_from(row.try_get::<i64, _>("plaintext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                ciphertext_size: u64::try_from(row.try_get::<i64, _>("ciphertext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                deduplicated: true,
                warnings: Vec::new(),
            }),
        ));
    }
    let warnings = detect_secret_warnings(&body);
    let (object_key, encrypted) = store.put_encrypted(scope, &digest_hex, &body).await?;
    let ciphertext_size = encrypted.ciphertext.len() as u64;
    sqlx::query(
        r#"
        INSERT INTO snapshot_chunks (
            key_scope_type, key_scope_id, plaintext_digest, object_key,
            plaintext_size, ciphertext_size, wrapped_data_key, nonce,
            wrap_nonce, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ready')
        "#,
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .bind(object_key)
    .bind(i64::try_from(body.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(ciphertext_size).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .execute(&mut *tx)
    .await?;
    insert_chunk_reservation(&mut tx, manifest_id, scope, &digest).await?;
    if !warnings.is_empty() {
        sqlx::query("UPDATE snapshot_manifests SET warnings = warnings || $2::jsonb WHERE id = $1")
            .bind(manifest_id)
            .bind(serde_json::to_value(&warnings)?)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(SnapshotChunkReceipt {
            schema_version: SCHEMA_VERSION,
            manifest_id,
            digest: digest_hex,
            plaintext_size: body.len() as u64,
            ciphertext_size,
            deduplicated: false,
            warnings,
        }),
    ))
}

pub async fn commit_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotManifest>, ApiError> {
    require_store(&state)?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 FOR UPDATE")
        .bind(manifest_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    let project_id: Uuid = row.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    let status: String = row.try_get("status")?;
    if status == "ready" {
        return Ok(Json(snapshot_from_row(&state.pool, &row).await?));
    }
    if status != "uploading" {
        return Err(ApiError::Conflict(
            "snapshot is not in an uploadable state".to_owned(),
        ));
    }
    let scope = row_scope(&row)?;
    let upload_files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let unique = unique_digests(&upload_files)?;
    let chunk_rows = fetch_chunk_rows(&mut tx, scope, unique.keys()).await?;
    if chunk_rows.len() != unique.len() {
        let present: HashSet<&str> = chunk_rows.keys().map(String::as_str).collect();
        let missing: Vec<&str> = unique
            .keys()
            .map(String::as_str)
            .filter(|digest| !present.contains(digest))
            .take(20)
            .collect();
        return Err(ApiError::Conflict(format!(
            "snapshot still has missing chunks: {}",
            missing.join(", ")
        )));
    }

    let mut mappings = Vec::new();
    let mut files = Vec::with_capacity(upload_files.len());
    for file in upload_files {
        let mut chunks = Vec::with_capacity(file.chunks.len());
        for (index, requested) in file.chunks.into_iter().enumerate() {
            let stored = chunk_rows.get(&requested.digest).ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("validated snapshot chunk disappeared"))
            })?;
            if stored.plaintext_size != requested.plaintext_size {
                return Err(ApiError::Conflict(format!(
                    "stored chunk {} has a different plaintext size",
                    requested.digest
                )));
            }
            mappings.push((
                file.path.clone(),
                i32::try_from(index).map_err(|error| ApiError::Internal(error.into()))?,
                hex::decode(&requested.digest)
                    .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid validated digest")))?,
            ));
            chunks.push(SnapshotChunk {
                digest: requested.digest,
                plaintext_size: requested.plaintext_size,
                ciphertext_size: stored.ciphertext_size,
            });
        }
        files.push(SnapshotFile {
            path: file.path,
            size: file.size,
            mode: file.mode,
            modified_at: file.modified_at,
            chunks,
        });
    }
    if !mappings.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO snapshot_manifest_chunks (manifest_id, path, chunk_index, key_scope_type, key_scope_id, plaintext_digest) ",
        );
        query.push_values(mappings, |mut values, (path, index, digest)| {
            values
                .push_bind(manifest_id)
                .push_bind(path)
                .push_bind(index)
                .push_bind(scope.kind())
                .push_bind(scope.id())
                .push_bind(digest);
        });
        query.push(" ON CONFLICT (manifest_id, path, chunk_index) DO NOTHING");
        query.build().execute(&mut *tx).await?;
        sqlx::query(
            r#"
            UPDATE snapshot_chunks chunk
            SET ref_count = chunk.ref_count + refs.count,
                last_referenced_at = now()
            FROM (
                SELECT key_scope_type, key_scope_id, plaintext_digest,
                       count(*)::bigint AS count
                FROM snapshot_manifest_chunks WHERE manifest_id = $1
                GROUP BY key_scope_type, key_scope_id, plaintext_digest
            ) refs
            WHERE chunk.key_scope_type = refs.key_scope_type
              AND chunk.key_scope_id = refs.key_scope_id
              AND chunk.plaintext_digest = refs.plaintext_digest
            "#,
        )
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    }
    let row = sqlx::query(
        "UPDATE snapshot_manifests SET status = 'ready', committed_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(manifest_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    let manifest = SnapshotManifest {
        schema_version: SCHEMA_VERSION,
        id: manifest_id,
        project_id,
        total_bytes: u64::try_from(row.try_get::<i64, _>("total_bytes")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        files,
        encryption_key_id: row.try_get("encryption_key_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    };
    tx.commit().await?;
    resume_snapshot_runs(&state, manifest_id).await?;
    Ok(Json(manifest))
}

pub async fn get_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotManifest>, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Viewer,
    )
    .await?;
    if row.try_get::<String, _>("status")? != "ready" {
        return Err(ApiError::Conflict("snapshot is not ready".to_owned()));
    }
    Ok(Json(snapshot_from_row(&state.pool, &row).await?))
}

pub async fn download_snapshot_chunk(
    State(state): State<AppState>,
    Path((manifest_id, digest_hex)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<impl IntoResponse, ApiError> {
    let store = require_store(&state)?;
    validate_digest(&digest_hex)?;
    let manifest = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Viewer,
    )
    .await?;
    if manifest.try_get::<String, _>("status")? != "ready" {
        return Err(ApiError::Conflict("snapshot is not ready".to_owned()));
    }
    let plaintext =
        decrypt_manifest_chunk(&state.pool, &store, &manifest, manifest_id, &digest_hex).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        plaintext,
    ))
}

pub async fn get_executor_run_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<SnapshotManifest>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let manifest_id = run
        .spec
        .snapshot_id
        .ok_or_else(|| ApiError::NotFound("the run has no snapshot".to_owned()))?;
    let row = sqlx::query(
        "SELECT * FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(manifest_id)
    .bind(run.spec.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    Ok(Json(snapshot_from_row(&state.pool, &row).await?))
}

pub async fn begin_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    Json(request): Json<BeginSnapshotUploadRequest>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    if request.project_id != run.spec.project_id {
        return Err(ApiError::Unprocessable(
            "a result snapshot must belong to the leased run project".to_owned(),
        ));
    }
    begin_snapshot_upload_for(&state, run.spec.creator_user_id, request, Some(run_id)).await
}

pub async fn upload_executor_run_result_chunk(
    State(state): State<AppState>,
    Path((executor_id, run_id, manifest_id, digest_hex)): Path<(Uuid, Uuid, Uuid, String)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<SnapshotChunkReceipt>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    ensure_run_result_manifest(&state.pool, manifest_id, &run).await?;
    upload_snapshot_chunk(
        State(state),
        Path((manifest_id, digest_hex)),
        Extension(Principal {
            user_id: run.spec.creator_user_id,
            session_id: None,
            bootstrap: false,
        }),
        body,
    )
    .await
}

pub async fn get_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<Option<SnapshotManifest>>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let row = sqlx::query(
        r#"
        SELECT * FROM snapshot_manifests
        WHERE source_run_id = $1 AND project_id = $2 AND created_by = $3
          AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())
        "#,
    )
    .bind(run_id)
    .bind(run.spec.project_id)
    .bind(run.spec.creator_user_id)
    .fetch_optional(&state.pool)
    .await?;
    match row {
        Some(row) => Ok(Json(Some(snapshot_from_row(&state.pool, &row).await?))),
        None => Ok(Json(None)),
    }
}

pub async fn commit_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id, manifest_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<SnapshotManifest>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    ensure_run_result_manifest(&state.pool, manifest_id, &run).await?;
    commit_snapshot(
        State(state),
        Path(manifest_id),
        Extension(Principal {
            user_id: run.spec.creator_user_id,
            session_id: None,
            bootstrap: false,
        }),
    )
    .await
}

pub async fn download_executor_run_chunk(
    State(state): State<AppState>,
    Path((executor_id, run_id, digest_hex)): Path<(Uuid, Uuid, String)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    validate_digest(&digest_hex)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let manifest_id = run
        .spec
        .snapshot_id
        .ok_or_else(|| ApiError::NotFound("the run has no snapshot".to_owned()))?;
    let manifest = sqlx::query(
        "SELECT * FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(manifest_id)
    .bind(run.spec.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    let store = require_store(&state)?;
    let plaintext =
        decrypt_manifest_chunk(&state.pool, &store, &manifest, manifest_id, &digest_hex).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        plaintext,
    ))
}

pub async fn upload_executor_run_artifact(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ExecutorArtifactQuery>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    validate_executor_artifact_path(&query.path)?;
    if query.source.trim().is_empty() || query.source.len() > 100 {
        return Err(ApiError::Unprocessable(
            "artifact source must contain 1 to 100 characters".to_owned(),
        ));
    }
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let store = require_store(&state)?;
    let payload = persist_run_artifact(
        &state.pool,
        &store,
        run_id,
        run.spec.project_id,
        run.spec.creator_user_id,
        &query.path,
        &query.source,
        query.source_event_id,
        &body,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    match query.source_event_id {
        Some(event_id) => {
            db::append_event_tx_with_id(
                &mut tx,
                run_id,
                event_id,
                RunEventKind::ArtifactCreated,
                payload.clone(),
            )
            .await?;
        }
        None => {
            db::append_event_tx(
                &mut tx,
                run_id,
                RunEventKind::ArtifactCreated,
                payload.clone(),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(payload)))
}

async fn decrypt_manifest_chunk(
    pool: &PgPool,
    store: &ObjectStore,
    manifest: &sqlx::postgres::PgRow,
    manifest_id: Uuid,
    digest_hex: &str,
) -> Result<Vec<u8>, ApiError> {
    let scope = row_scope(manifest)?;
    let digest = hex::decode(digest_hex)
        .map_err(|_| ApiError::Unprocessable("invalid chunk digest".to_owned()))?;
    let row = sqlx::query(
        r#"
        SELECT chunk.* FROM snapshot_chunks chunk
        JOIN snapshot_manifest_chunks mapping
          ON mapping.key_scope_type = chunk.key_scope_type
         AND mapping.key_scope_id = chunk.key_scope_id
         AND mapping.plaintext_digest = chunk.plaintext_digest
        WHERE mapping.manifest_id = $1 AND chunk.plaintext_digest = $2
          AND chunk.status = 'ready' LIMIT 1
        "#,
    )
    .bind(manifest_id)
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("chunk {digest_hex} was not found")))?;
    let wrap_nonce: Option<Vec<u8>> = row.try_get("wrap_nonce")?;
    let plaintext = store
        .get_decrypted(
            scope,
            row.try_get("object_key")?,
            row.try_get("wrapped_data_key")?,
            row.try_get("nonce")?,
            wrap_nonce.as_deref().ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("chunk has no wrapped-key nonce"))
            })?,
        )
        .await?;
    if hex::encode(Sha256::digest(&plaintext)) != digest_hex {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "decrypted chunk digest verification failed"
        )));
    }
    Ok(plaintext)
}

fn ensure_executor_path(principal: &ExecutorPrincipal, executor_id: Uuid) -> Result<(), ApiError> {
    if principal.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential does not match the route".to_owned(),
        ));
    }
    Ok(())
}

fn executor_lease_token(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get("x-cowork-lease-token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing executor lease token".to_owned()))?
        .parse()
        .map_err(|_| ApiError::Unauthorized("invalid executor lease token".to_owned()))
}

async fn verify_executor_lease(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    headers: &HeaderMap,
) -> Result<cowork_contracts::RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    let run =
        db::verify_lease(&mut tx, run_id, executor_id, executor_lease_token(headers)?).await?;
    tx.commit().await?;
    Ok(run)
}

async fn ensure_run_result_manifest(
    pool: &PgPool,
    manifest_id: Uuid,
    run: &cowork_contracts::RunRecord,
) -> Result<(), ApiError> {
    let matches = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM snapshot_manifests
            WHERE id = $1 AND source_run_id = $2 AND project_id = $3
              AND created_by = $4
        )
        "#,
    )
    .bind(manifest_id)
    .bind(run.spec.id)
    .bind(run.spec.project_id)
    .bind(run.spec.creator_user_id)
    .fetch_one(pool)
    .await?;
    if !matches {
        return Err(ApiError::NotFound(
            "the result snapshot does not belong to this leased run".to_owned(),
        ));
    }
    Ok(())
}

fn validate_executor_artifact_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty() || path.len() > 1_024 || path.contains('\\') {
        return Err(ApiError::Unprocessable(
            "artifact path is invalid".to_owned(),
        ));
    }
    let mut components = FsPath::new(path).components();
    if !matches!(components.next(), Some(Component::Normal(value)) if value == "artifacts")
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ApiError::Unprocessable(
            "artifact path must stay below artifacts/".to_owned(),
        ));
    }
    Ok(())
}

fn validate_attachment_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::Unprocessable(
            "attachment name must be a single safe file name".to_owned(),
        ));
    }
    Ok(())
}

pub async fn delete_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Editor,
    )
    .await?;
    let used = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_versions WHERE snapshot_manifest_id = $1)",
    )
    .bind(manifest_id)
    .fetch_one(&state.pool)
    .await?;
    if used {
        return Err(ApiError::Conflict(
            "snapshot is retained by a project version".to_owned(),
        ));
    }
    if row.try_get::<String, _>("status")? == "ready" {
        release_manifest_references(&state.pool, manifest_id).await?;
    }
    sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1")
        .bind(manifest_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_project_version(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<cowork_contracts::CreateProjectVersionRequest>,
) -> Result<(StatusCode, Json<ProjectVersion>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let snapshot_ready = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready')",
    )
    .bind(request.snapshot_manifest_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if !snapshot_ready {
        return Err(ApiError::Unprocessable(
            "project versions require a ready snapshot from the same project".to_owned(),
        ));
    }
    validate_version_parent(&mut tx, project_id, request.parent_version_id).await?;
    validate_version_parent(&mut tx, project_id, request.merge_base_version_id).await?;
    if let Some(run_id) = request.created_by_run_id {
        let valid_run = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = $1 AND project_id = $2 AND state = 'completed')",
        )
        .bind(run_id)
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        if !valid_run {
            return Err(ApiError::Unprocessable(
                "created_by_run_id must be a completed run from this project".to_owned(),
            ));
        }
    }
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM project_versions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO project_versions (
            id, project_id, revision, parent_version_id, merge_base_version_id,
            snapshot_manifest_id, created_by_user_id, created_by_run_id,
            diff_summary
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(revision)
    .bind(request.parent_version_id)
    .bind(request.merge_base_version_id)
    .bind(request.snapshot_manifest_id)
    .bind(principal.user_id)
    .bind(request.created_by_run_id)
    .bind(request.diff_summary)
    .fetch_one(&mut *tx)
    .await?;
    let version = row_to_project_version(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(version)))
}

pub async fn list_project_versions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ProjectVersion>>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let rows =
        sqlx::query("SELECT * FROM project_versions WHERE project_id = $1 ORDER BY revision DESC")
            .bind(project_id)
            .fetch_all(&state.pool)
            .await?;
    rows.iter()
        .map(row_to_project_version)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn apply_project_version(
    State(state): State<AppState>,
    Path((project_id, version_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ApplyProjectVersionRequest>,
) -> Result<Json<ProjectVersion>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let project = sqlx::query(
        "SELECT revision, current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let revision: i64 = project.try_get("revision")?;
    let current: Option<Uuid> = project.try_get("current_version_id")?;
    if revision != request.expected_project_revision
        || current != request.expected_current_version_id
    {
        return Err(ApiError::Conflict(
            "project changed since the review started".to_owned(),
        ));
    }
    let version = sqlx::query("SELECT * FROM project_versions WHERE id = $1 AND project_id = $2")
        .bind(version_id)
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project version {version_id} was not found")))?;
    let next_revision = revision + 1;
    sqlx::query(
        "UPDATE projects SET current_version_id = $2, revision = $3, etag = $4, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(version_id)
    .bind(next_revision)
    .bind(format!("W/\"{project_id}:{next_revision}\""))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'project.version.apply', 'project', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(project_id)
    .bind(json!({"version_id": version_id, "previous_version_id": current}))
    .execute(&mut *tx)
    .await?;
    let record = row_to_project_version(&version)?;
    tx.commit().await?;
    Ok(Json(record))
}

pub async fn review_project_merge(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<MergeReviewQuery>,
) -> Result<Json<ProjectMergeReview>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let store = require_store(&state)?;
    let (base, current, result) = load_merge_versions(
        &state.pool,
        project_id,
        query.base_version_id,
        query.current_version_id,
        query.result_version_id,
    )
    .await?;
    let analysis = analyze_merge(&state.pool, &store, &base, &current, &result).await?;
    Ok(Json(ProjectMergeReview {
        schema_version: SCHEMA_VERSION,
        project_id,
        base_version_id: base.version.id,
        current_version_id: current.version.id,
        result_version_id: result.version.id,
        files: analysis.into_iter().map(|item| item.review).collect(),
    }))
}

pub async fn apply_project_merge(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ApplyProjectMergeRequest>,
) -> Result<(StatusCode, Json<ProjectVersion>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let store = require_store(&state)?;
    let (base, current, result) = load_merge_versions(
        &state.pool,
        project_id,
        request.base_version_id,
        request.current_version_id,
        request.result_version_id,
    )
    .await?;
    let analysis = analyze_merge(&state.pool, &store, &base, &current, &result).await?;
    let valid_paths: HashSet<&str> = analysis
        .iter()
        .map(|item| item.review.path.as_str())
        .collect();
    let mut resolutions = HashMap::new();
    for resolution in request.resolutions {
        if !valid_paths.contains(resolution.path.as_str()) {
            return Err(ApiError::Unprocessable(format!(
                "merge resolution references unknown path {}",
                resolution.path
            )));
        }
        if resolutions
            .insert(resolution.path.clone(), resolution.choice)
            .is_some()
        {
            return Err(ApiError::Unprocessable(format!(
                "merge resolution for {} is duplicated",
                resolution.path
            )));
        }
    }
    let scope_row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1")
        .bind(current.manifest_id)
        .fetch_one(&state.pool)
        .await?;
    let scope = row_scope(&scope_row)?;
    let mut output = Vec::new();
    for item in &analysis {
        let path = &item.review.path;
        let explicit = resolutions.get(path).copied();
        let choice = explicit.unwrap_or_else(|| default_resolution(item.review.status));
        let selected = match choice {
            MergeResolutionChoice::Current => current.files.get(path).cloned(),
            MergeResolutionChoice::Result => result.files.get(path).cloned(),
            MergeResolutionChoice::Delete => None,
            MergeResolutionChoice::AutoMerged => {
                let bytes = item.auto_merged.as_ref().ok_or_else(|| {
                    ApiError::Unprocessable(format!("{path} has no conflict-free automatic merge"))
                })?;
                let template = result
                    .files
                    .get(path)
                    .or_else(|| current.files.get(path))
                    .ok_or_else(|| {
                        ApiError::Internal(anyhow::anyhow!("merge template file disappeared"))
                    })?;
                Some(upload_file_from_bytes(&state.pool, &store, scope, template, bytes).await?)
            }
        };
        if matches!(
            item.review.status,
            MergeFileStatus::TextConflict | MergeFileStatus::BinaryConflict
        ) && explicit.is_none()
        {
            return Err(ApiError::Unprocessable(format!(
                "conflict for {path} requires an explicit resolution"
            )));
        }
        if let Some(file) = selected {
            output.push(snapshot_to_upload_file(file));
        }
    }
    output.sort_by(|left, right| left.path.cmp(&right.path));
    let merged_manifest_id =
        create_ready_manifest(&state.pool, project_id, principal.user_id, scope, &output).await?;

    let mut tx = state.pool.begin().await?;
    let project = sqlx::query(
        "SELECT revision, current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let project_revision: i64 = project.try_get("revision")?;
    let current_version_id: Option<Uuid> = project.try_get("current_version_id")?;
    if project_revision != request.expected_project_revision
        || current_version_id != Some(current.version.id)
    {
        return Err(ApiError::Conflict(
            "project changed since the merge review was generated".to_owned(),
        ));
    }
    let version_revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM project_versions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    let version_id = Uuid::new_v4();
    let review_summary = json!({
        "base_version_id": base.version.id,
        "current_version_id": current.version.id,
        "result_version_id": result.version.id,
        "files": analysis.iter().map(|item| &item.review).collect::<Vec<_>>()
    });
    let version_row = sqlx::query(
        r#"
        INSERT INTO project_versions (
            id, project_id, revision, parent_version_id, merge_base_version_id,
            snapshot_manifest_id, created_by_user_id, diff_summary
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *
        "#,
    )
    .bind(version_id)
    .bind(project_id)
    .bind(version_revision)
    .bind(current.version.id)
    .bind(base.version.id)
    .bind(merged_manifest_id)
    .bind(principal.user_id)
    .bind(review_summary)
    .fetch_one(&mut *tx)
    .await?;
    let next_project_revision = project_revision + 1;
    sqlx::query(
        "UPDATE projects SET current_version_id = $2, revision = $3, etag = $4, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(version_id)
    .bind(next_project_revision)
    .bind(format!("W/\"{project_id}:{next_project_revision}\""))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'project.merge.apply', 'project', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(project_id)
    .bind(json!({"version_id": version_id, "result_version_id": result.version.id}))
    .execute(&mut *tx)
    .await?;
    let version = row_to_project_version(&version_row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(version)))
}

async fn load_merge_versions(
    pool: &PgPool,
    project_id: Uuid,
    base_id: Uuid,
    current_id: Uuid,
    result_id: Uuid,
) -> Result<(VersionFiles, VersionFiles, VersionFiles), ApiError> {
    let project_current = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .ok_or_else(|| ApiError::Conflict("project has no current version".to_owned()))?;
    if project_current != current_id {
        return Err(ApiError::Conflict(
            "current_version_id is no longer the project's current version".to_owned(),
        ));
    }
    let base = load_version_files(pool, project_id, base_id).await?;
    let current = load_version_files(pool, project_id, current_id).await?;
    let result = load_version_files(pool, project_id, result_id).await?;
    Ok((base, current, result))
}

async fn load_version_files(
    pool: &PgPool,
    project_id: Uuid,
    version_id: Uuid,
) -> Result<VersionFiles, ApiError> {
    let row = sqlx::query("SELECT * FROM project_versions WHERE id = $1 AND project_id = $2")
        .bind(version_id)
        .bind(project_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project version {version_id} was not found")))?;
    let version = row_to_project_version(&row)?;
    let manifest_row =
        sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 AND status = 'ready'")
            .bind(version.snapshot_manifest_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| {
                ApiError::Conflict(format!("version {version_id} snapshot is not ready"))
            })?;
    let manifest = snapshot_from_row(pool, &manifest_row).await?;
    Ok(VersionFiles {
        version,
        manifest_id: manifest.id,
        files: manifest
            .files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect(),
    })
}

async fn analyze_merge(
    pool: &PgPool,
    store: &ObjectStore,
    base: &VersionFiles,
    current: &VersionFiles,
    result: &VersionFiles,
) -> Result<Vec<FileAnalysis>, ApiError> {
    let mut paths: HashSet<String> = base.files.keys().cloned().collect();
    paths.extend(current.files.keys().cloned());
    paths.extend(result.files.keys().cloned());
    let mut paths: Vec<String> = paths.into_iter().collect();
    paths.sort();
    let mut analysis = Vec::with_capacity(paths.len());
    for path in paths {
        let base_file = base.files.get(&path);
        let current_file = current.files.get(&path);
        let result_file = result.files.get(&path);
        let base_digest = base_file.map(file_fingerprint);
        let current_digest = current_file.map(file_fingerprint);
        let result_digest = result_file.map(file_fingerprint);
        let mut auto_merged = None;
        let mut conflict_preview = None;
        let status = match (
            base_digest.as_ref(),
            current_digest.as_ref(),
            result_digest.as_ref(),
        ) {
            (None, None, Some(_)) => MergeFileStatus::Added,
            (None, Some(_), None) => MergeFileStatus::CurrentOnly,
            (None, Some(current), Some(result)) if current == result => {
                MergeFileStatus::IdenticalChange
            }
            (None, Some(_), Some(_)) => MergeFileStatus::BinaryConflict,
            (Some(base), Some(current), Some(result)) if base == current && base == result => {
                MergeFileStatus::Unchanged
            }
            (Some(_), None, None) => MergeFileStatus::Deleted,
            (Some(base), Some(current), None) if base == current => MergeFileStatus::Deleted,
            (Some(_), Some(_), None) => MergeFileStatus::BinaryConflict,
            (Some(base), None, Some(result)) if base == result => MergeFileStatus::CurrentOnly,
            (Some(_), None, Some(_)) => MergeFileStatus::BinaryConflict,
            (Some(base), Some(current), Some(_)) if base == current => MergeFileStatus::ResultOnly,
            (Some(base), Some(_), Some(result)) if base == result => MergeFileStatus::CurrentOnly,
            (Some(_), Some(current), Some(result)) if current == result => {
                MergeFileStatus::IdenticalChange
            }
            (Some(_), Some(_), Some(_)) => {
                match attempt_text_merge(
                    pool,
                    store,
                    base.manifest_id,
                    current.manifest_id,
                    result.manifest_id,
                    &path,
                    base_file.expect("matched Some"),
                    current_file.expect("matched Some"),
                    result_file.expect("matched Some"),
                )
                .await?
                {
                    TextMerge::Merged(bytes) => {
                        auto_merged = Some(bytes);
                        MergeFileStatus::AutoMerged
                    }
                    TextMerge::Conflict(preview) => {
                        conflict_preview = Some(preview);
                        MergeFileStatus::TextConflict
                    }
                    TextMerge::Binary => MergeFileStatus::BinaryConflict,
                }
            }
            (None, None, None) => unreachable!(),
        };
        analysis.push(FileAnalysis {
            review: MergeFileReview {
                path,
                renamed_from: None,
                status,
                base_digest,
                current_digest,
                result_digest,
                auto_mergeable: status == MergeFileStatus::AutoMerged,
                conflict_preview,
            },
            auto_merged,
        });
    }
    annotate_renames(&mut analysis, base, result);
    Ok(analysis)
}

enum TextMerge {
    Merged(Vec<u8>),
    Conflict(String),
    Binary,
}

#[allow(clippy::too_many_arguments)]
async fn attempt_text_merge(
    pool: &PgPool,
    store: &ObjectStore,
    base_manifest: Uuid,
    current_manifest: Uuid,
    result_manifest: Uuid,
    path: &str,
    base: &SnapshotFile,
    current: &SnapshotFile,
    result: &SnapshotFile,
) -> Result<TextMerge, ApiError> {
    if [base.size, current.size, result.size]
        .into_iter()
        .any(|size| size > MAX_TEXT_MERGE_BYTES)
    {
        return Ok(TextMerge::Binary);
    }
    let base = materialize_file(pool, store, base_manifest, path).await?;
    let current = materialize_file(pool, store, current_manifest, path).await?;
    let result = materialize_file(pool, store, result_manifest, path).await?;
    let (Ok(base), Ok(current), Ok(result)) = (
        String::from_utf8(base),
        String::from_utf8(current),
        String::from_utf8(result),
    ) else {
        return Ok(TextMerge::Binary);
    };
    match diffy::merge(&base, &current, &result) {
        Ok(merged) => Ok(TextMerge::Merged(merged.into_bytes())),
        Err(conflict) => Ok(TextMerge::Conflict(conflict.chars().take(20_000).collect())),
    }
}

fn annotate_renames(analysis: &mut [FileAnalysis], base: &VersionFiles, result: &VersionFiles) {
    let deleted: Vec<(&String, String)> = base
        .files
        .iter()
        .filter(|(path, _)| !result.files.contains_key(*path))
        .map(|(path, file)| (path, file_fingerprint(file)))
        .collect();
    for item in analysis {
        if item.review.status != MergeFileStatus::Added {
            continue;
        }
        let Some(result_file) = result.files.get(&item.review.path) else {
            continue;
        };
        let fingerprint = file_fingerprint(result_file);
        if let Some((old_path, _)) = deleted.iter().find(|(_, old)| *old == fingerprint) {
            item.review.status = MergeFileStatus::Renamed;
            item.review.renamed_from = Some((*old_path).to_owned());
        }
    }
}

fn default_resolution(status: MergeFileStatus) -> MergeResolutionChoice {
    match status {
        MergeFileStatus::Unchanged | MergeFileStatus::CurrentOnly => MergeResolutionChoice::Current,
        MergeFileStatus::AutoMerged => MergeResolutionChoice::AutoMerged,
        MergeFileStatus::Deleted => MergeResolutionChoice::Result,
        MergeFileStatus::Added
        | MergeFileStatus::ResultOnly
        | MergeFileStatus::IdenticalChange
        | MergeFileStatus::Renamed => MergeResolutionChoice::Result,
        MergeFileStatus::TextConflict | MergeFileStatus::BinaryConflict => {
            MergeResolutionChoice::Current
        }
    }
}

async fn materialize_file(
    pool: &PgPool,
    store: &ObjectStore,
    manifest_id: Uuid,
    path: &str,
) -> Result<Vec<u8>, ApiError> {
    let manifest =
        sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 AND status = 'ready'")
            .bind(manifest_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::Conflict("snapshot is not ready".to_owned()))?;
    let scope = row_scope(&manifest)?;
    let rows = sqlx::query(
        r#"
        SELECT chunk.* FROM snapshot_manifest_chunks mapping
        JOIN snapshot_chunks chunk
          ON chunk.key_scope_type = mapping.key_scope_type
         AND chunk.key_scope_id = mapping.key_scope_id
         AND chunk.plaintext_digest = mapping.plaintext_digest
        WHERE mapping.manifest_id = $1 AND mapping.path = $2
        ORDER BY mapping.chunk_index
        "#,
    )
    .bind(manifest_id)
    .bind(path)
    .fetch_all(pool)
    .await?;
    let mut output = Vec::new();
    for row in rows {
        let wrap_nonce: Option<Vec<u8>> = row.try_get("wrap_nonce")?;
        let chunk = store
            .get_decrypted(
                scope,
                row.try_get("object_key")?,
                row.try_get("wrapped_data_key")?,
                row.try_get("nonce")?,
                wrap_nonce.as_deref().ok_or_else(|| {
                    ApiError::Internal(anyhow::anyhow!("chunk has no wrapped-key nonce"))
                })?,
            )
            .await?;
        output.extend_from_slice(&chunk);
        if output.len() as u64 > MAX_TEXT_MERGE_BYTES {
            return Err(ApiError::Unprocessable(
                "text merge materialization exceeded its size limit".to_owned(),
            ));
        }
    }
    Ok(output)
}

async fn upload_file_from_bytes(
    pool: &PgPool,
    store: &ObjectStore,
    scope: KeyScope,
    template: &SnapshotFile,
    bytes: &[u8],
) -> Result<SnapshotFile, ApiError> {
    let mut chunks = Vec::new();
    for part in bytes.chunks(MAX_CHUNK_BYTES) {
        let digest = hex::encode(Sha256::digest(part));
        let ciphertext_size = persist_plaintext_chunk(pool, store, scope, &digest, part).await?;
        chunks.push(SnapshotChunk {
            digest,
            plaintext_size: part.len() as u64,
            ciphertext_size,
        });
    }
    Ok(SnapshotFile {
        path: template.path.clone(),
        size: bytes.len() as u64,
        mode: template.mode,
        modified_at: Utc::now(),
        chunks,
    })
}

async fn persist_plaintext_chunk(
    pool: &PgPool,
    store: &ObjectStore,
    scope: KeyScope,
    digest_hex: &str,
    bytes: &[u8],
) -> Result<u64, ApiError> {
    let digest = hex::decode(digest_hex)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid generated digest")))?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{}:{}:{digest_hex}", scope.kind(), scope.id()))
        .execute(&mut *tx)
        .await?;
    if let Some(size) = sqlx::query_scalar::<_, i64>(
        "SELECT ciphertext_size FROM snapshot_chunks WHERE key_scope_type = $1 AND key_scope_id = $2 AND plaintext_digest = $3 AND status = 'ready'",
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return u64::try_from(size).map_err(|error| ApiError::Internal(error.into()));
    }
    let (object_key, encrypted) = store.put_encrypted(scope, digest_hex, bytes).await?;
    let ciphertext_size = encrypted.ciphertext.len() as u64;
    sqlx::query(
        r#"
        INSERT INTO snapshot_chunks (
            key_scope_type, key_scope_id, plaintext_digest, object_key,
            plaintext_size, ciphertext_size, wrapped_data_key, nonce,
            wrap_nonce, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ready')
        "#,
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(digest)
    .bind(object_key)
    .bind(i64::try_from(bytes.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(ciphertext_size).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ciphertext_size)
}

async fn create_ready_manifest(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
    scope: KeyScope,
    files: &[SnapshotUploadFile],
) -> Result<Uuid, ApiError> {
    let total_bytes = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.size)
            .ok_or_else(|| ApiError::Unprocessable("merged snapshot size overflow".to_owned()))
    })?;
    validate_snapshot_files(total_bytes, files)?;
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO snapshot_manifests (
            id, project_id, created_by, key_scope_type, key_scope_id,
            encryption_key_id, total_bytes, file_count, manifest, status,
            expires_at, committed_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ready', $10, now())
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(user_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(scope.key_id())
    .bind(i64::try_from(total_bytes).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(files.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(serde_json::to_value(files)?)
    // If the final project transaction loses an optimistic-concurrency race,
    // this unattached prepared manifest becomes collectible after one day.
    .bind(Utc::now() + Duration::days(1))
    .execute(&mut *tx)
    .await?;
    let mut mappings = Vec::new();
    for file in files {
        for (index, chunk) in file.chunks.iter().enumerate() {
            mappings.push((
                file.path.clone(),
                i32::try_from(index).map_err(|error| ApiError::Internal(error.into()))?,
                hex::decode(&chunk.digest)
                    .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid merged digest")))?,
            ));
        }
    }
    if !mappings.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO snapshot_manifest_chunks (manifest_id, path, chunk_index, key_scope_type, key_scope_id, plaintext_digest) ",
        );
        query.push_values(mappings, |mut values, (path, index, digest)| {
            values
                .push_bind(id)
                .push_bind(path)
                .push_bind(index)
                .push_bind(scope.kind())
                .push_bind(scope.id())
                .push_bind(digest);
        });
        query.build().execute(&mut *tx).await?;
        sqlx::query(
            r#"
            UPDATE snapshot_chunks chunk SET ref_count = chunk.ref_count + refs.count,
                last_referenced_at = now()
            FROM (
                SELECT key_scope_type, key_scope_id, plaintext_digest,
                       count(*)::bigint AS count
                FROM snapshot_manifest_chunks WHERE manifest_id = $1
                GROUP BY key_scope_type, key_scope_id, plaintext_digest
            ) refs
            WHERE chunk.key_scope_type = refs.key_scope_type
              AND chunk.key_scope_id = refs.key_scope_id
              AND chunk.plaintext_digest = refs.plaintext_digest
            "#,
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

fn snapshot_to_upload_file(file: SnapshotFile) -> SnapshotUploadFile {
    SnapshotUploadFile {
        path: file.path,
        size: file.size,
        mode: file.mode,
        modified_at: file.modified_at,
        chunks: file
            .chunks
            .into_iter()
            .map(|chunk| SnapshotUploadChunk {
                digest: chunk.digest,
                plaintext_size: chunk.plaintext_size,
            })
            .collect(),
    }
}

fn file_fingerprint(file: &SnapshotFile) -> String {
    let mut hash = Sha256::new();
    hash.update(file.size.to_be_bytes());
    for chunk in &file.chunks {
        hash.update(chunk.digest.as_bytes());
        hash.update(chunk.plaintext_size.to_be_bytes());
    }
    hex::encode(hash.finalize())
}

pub async fn garbage_collect(
    pool: &PgPool,
    store: &ObjectStore,
    limit: i64,
) -> Result<usize, ApiError> {
    let abandoned = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM snapshot_manifests WHERE status = 'uploading' AND upload_expires_at <= now() ORDER BY upload_expires_at LIMIT $1",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    for manifest_id in abandoned {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
            .bind(manifest_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1 AND status = 'uploading'")
            .bind(manifest_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    let expired = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT manifest.id FROM snapshot_manifests manifest
        WHERE manifest.status = 'ready' AND manifest.expires_at <= now()
          AND NOT EXISTS(
              SELECT 1 FROM project_versions version
              WHERE version.snapshot_manifest_id = manifest.id
          )
        ORDER BY manifest.expires_at LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    for manifest_id in expired {
        release_manifest_references(pool, manifest_id).await?;
        sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1")
            .bind(manifest_id)
            .execute(pool)
            .await?;
    }
    let rows = sqlx::query(
        r#"
        UPDATE snapshot_chunks SET status = 'deleting'
        WHERE (key_scope_type, key_scope_id, plaintext_digest) IN (
            SELECT key_scope_type, key_scope_id, plaintext_digest
            FROM snapshot_chunks chunk
            WHERE ref_count = 0 AND status = 'ready'
              AND NOT EXISTS (
                  SELECT 1 FROM snapshot_chunk_reservations reservation
                  WHERE reservation.key_scope_type = chunk.key_scope_type
                    AND reservation.key_scope_id = chunk.key_scope_id
                    AND reservation.plaintext_digest = chunk.plaintext_digest
              )
            ORDER BY last_referenced_at LIMIT $1 FOR UPDATE SKIP LOCKED
        ) RETURNING key_scope_type, key_scope_id, plaintext_digest, object_key
        "#,
    )
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await?;
    let mut deleted = 0;
    for row in rows {
        let object_key: String = row.try_get("object_key")?;
        match store.delete(&object_key).await {
            Ok(()) => {
                sqlx::query(
                    "DELETE FROM snapshot_chunks WHERE key_scope_type = $1 AND key_scope_id = $2 AND plaintext_digest = $3 AND ref_count = 0 AND status = 'deleting'",
                )
                .bind(row.try_get::<String, _>("key_scope_type")?)
                .bind(row.try_get::<Uuid, _>("key_scope_id")?)
                .bind(row.try_get::<Vec<u8>, _>("plaintext_digest")?)
                .execute(pool)
                .await?;
                deleted += 1;
            }
            Err(error) => {
                tracing::warn!(?error, %object_key, "failed to delete unreferenced object-store chunk");
                sqlx::query(
                    "UPDATE snapshot_chunks SET status = 'ready' WHERE object_key = $1 AND status = 'deleting'",
                )
                .bind(object_key)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(deleted)
}

async fn resume_snapshot_runs(state: &AppState, manifest_id: Uuid) -> Result<(), ApiError> {
    let run_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM runs WHERE snapshot_id = $1 AND state = 'waiting_for_snapshot'",
    )
    .bind(manifest_id)
    .fetch_all(&state.pool)
    .await?;
    for run_id in run_ids {
        let run = db::get_run(&state.pool, run_id).await?;
        let available = match &run.spec.executor_target {
            ExecutorTarget::ServerLinux { .. } => {
                let server: HashSet<&str> = state
                    .server_capabilities
                    .iter()
                    .map(|capability| capability.0.as_str())
                    .collect();
                run.spec
                    .required_capabilities
                    .iter()
                    .all(|required| server.contains(required.0.as_str()))
            }
            target => {
                db::target_has_executor(
                    &state.pool,
                    target,
                    &run.spec.required_capabilities,
                    &run.spec.input,
                )
                .await?
            }
        };
        db::transition_run(
            &state.pool,
            run_id,
            if available {
                cowork_contracts::RunState::Queued
            } else {
                cowork_contracts::RunState::WaitingForExecutor
            },
            None,
            None,
        )
        .await?;
    }
    Ok(())
}

async fn release_manifest_references(pool: &PgPool, manifest_id: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE snapshot_chunks chunk
        SET ref_count = GREATEST(chunk.ref_count - refs.count, 0),
            last_referenced_at = now()
        FROM (
            SELECT key_scope_type, key_scope_id, plaintext_digest,
                   count(*)::bigint AS count
            FROM snapshot_manifest_chunks WHERE manifest_id = $1
            GROUP BY key_scope_type, key_scope_id, plaintext_digest
        ) refs
        WHERE chunk.key_scope_type = refs.key_scope_type
          AND chunk.key_scope_id = refs.key_scope_id
          AND chunk.plaintext_digest = refs.plaintext_digest
        "#,
    )
    .bind(manifest_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM snapshot_manifest_chunks WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn validate_version_parent(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    version_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(version_id) = version_id {
        let valid = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM project_versions WHERE id = $1 AND project_id = $2)",
        )
        .bind(version_id)
        .bind(project_id)
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(ApiError::Unprocessable(format!(
                "version {version_id} does not belong to this project"
            )));
        }
    }
    Ok(())
}

async fn accessible_manifest(
    pool: &PgPool,
    user_id: Uuid,
    manifest_id: Uuid,
    role: ProjectRole,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    let row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1")
        .bind(manifest_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    organization::ensure_project_role(pool, user_id, row.try_get("project_id")?, role).await?;
    Ok(row)
}

async fn project_scope(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<KeyScope, ApiError> {
    let row = sqlx::query(
        "SELECT privacy, owner_user_id, team_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    match row.try_get::<String, _>("privacy")?.as_str() {
        "private_local" => {
            let owner: Uuid = row.try_get("owner_user_id")?;
            if owner != user_id {
                return Err(ApiError::Unauthorized(
                    "private snapshots can only be uploaded by the project owner".to_owned(),
                ));
            }
            Ok(KeyScope::User(owner))
        }
        "team_managed" => Ok(KeyScope::Team(
            row.try_get::<Option<Uuid>, _>("team_id")?
                .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("team project has no team")))?,
        )),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "invalid project privacy in database: {other}"
        ))),
    }
}

fn row_scope(row: &sqlx::postgres::PgRow) -> Result<KeyScope, ApiError> {
    let id: Uuid = row.try_get("key_scope_id")?;
    match row.try_get::<String, _>("key_scope_type")?.as_str() {
        "user" => Ok(KeyScope::User(id)),
        "team" => Ok(KeyScope::Team(id)),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "invalid snapshot key scope in database: {other}"
        ))),
    }
}

fn validated_snapshot_expiry(
    scope: KeyScope,
    requested: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let now = Utc::now();
    match scope {
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

fn validate_snapshot_files(total_bytes: u64, files: &[SnapshotUploadFile]) -> Result<(), ApiError> {
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

fn validate_digest(digest: &str) -> Result<(), ApiError> {
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

fn unique_digests(files: &[SnapshotUploadFile]) -> Result<HashMap<String, u64>, ApiError> {
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

async fn existing_digests(
    pool: &PgPool,
    scope: KeyScope,
    digests: &HashMap<String, u64>,
) -> Result<HashSet<String>, ApiError> {
    if digests.is_empty() {
        return Ok(HashSet::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT encode(plaintext_digest, 'hex') AS digest FROM snapshot_chunks WHERE key_scope_type = ",
    );
    builder
        .push_bind(scope.kind())
        .push(" AND key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND status = 'ready' AND plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests.keys() {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(")");
    let rows = builder.build().fetch_all(pool).await?;
    rows.iter()
        .map(|row| row.try_get("digest").map_err(ApiError::from))
        .collect()
}

async fn reserve_existing_chunks<'a>(
    tx: &mut Transaction<'_, Postgres>,
    manifest_id: Uuid,
    scope: KeyScope,
    digests: impl Iterator<Item = &'a String>,
) -> Result<HashSet<String>, ApiError> {
    let digests: Vec<&String> = digests.collect();
    if digests.is_empty() {
        return Ok(HashSet::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "INSERT INTO snapshot_chunk_reservations (manifest_id, key_scope_type, key_scope_id, plaintext_digest) SELECT ",
    );
    builder
        .push_bind(manifest_id)
        .push(", chunk.key_scope_type, chunk.key_scope_id, chunk.plaintext_digest FROM snapshot_chunks chunk WHERE chunk.key_scope_type = ")
        .push_bind(scope.kind())
        .push(" AND chunk.key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND chunk.status = 'ready' AND chunk.plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(
        ") ON CONFLICT DO NOTHING RETURNING encode(plaintext_digest, 'hex') AS digest",
    );
    let rows = builder.build().fetch_all(&mut **tx).await?;
    rows.iter()
        .map(|row| row.try_get("digest").map_err(ApiError::from))
        .collect()
}

async fn insert_chunk_reservation(
    tx: &mut Transaction<'_, Postgres>,
    manifest_id: Uuid,
    scope: KeyScope,
    digest: &[u8],
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO snapshot_chunk_reservations (
            manifest_id, key_scope_type, key_scope_id, plaintext_digest
        ) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING
        "#,
    )
    .bind(manifest_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(digest)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

struct StoredChunk {
    plaintext_size: u64,
    ciphertext_size: u64,
}

async fn fetch_chunk_rows<'a>(
    tx: &mut Transaction<'_, Postgres>,
    scope: KeyScope,
    digests: impl Iterator<Item = &'a String>,
) -> Result<HashMap<String, StoredChunk>, ApiError> {
    let digests: Vec<&String> = digests.collect();
    if digests.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT encode(plaintext_digest, 'hex') AS digest, plaintext_size, ciphertext_size FROM snapshot_chunks WHERE key_scope_type = ",
    );
    builder
        .push_bind(scope.kind())
        .push(" AND key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND status = 'ready' AND plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(")");
    let rows = builder.build().fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("digest")?,
                StoredChunk {
                    plaintext_size: u64::try_from(row.try_get::<i64, _>("plaintext_size")?)
                        .map_err(|error| ApiError::Internal(error.into()))?,
                    ciphertext_size: u64::try_from(row.try_get::<i64, _>("ciphertext_size")?)
                        .map_err(|error| ApiError::Internal(error.into()))?,
                },
            ))
        })
        .collect()
}

async fn snapshot_from_row(
    pool: &PgPool,
    row: &sqlx::postgres::PgRow,
) -> Result<SnapshotManifest, ApiError> {
    let manifest_id: Uuid = row.try_get("id")?;
    let upload_files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let mappings = sqlx::query(
        r#"
        SELECT mapping.path, mapping.chunk_index,
               encode(chunk.plaintext_digest, 'hex') AS digest,
               chunk.plaintext_size, chunk.ciphertext_size
        FROM snapshot_manifest_chunks mapping
        JOIN snapshot_chunks chunk
          ON chunk.key_scope_type = mapping.key_scope_type
         AND chunk.key_scope_id = mapping.key_scope_id
         AND chunk.plaintext_digest = mapping.plaintext_digest
        WHERE mapping.manifest_id = $1
        ORDER BY mapping.path, mapping.chunk_index
        "#,
    )
    .bind(manifest_id)
    .fetch_all(pool)
    .await?;
    let mut by_path: HashMap<String, Vec<SnapshotChunk>> = HashMap::new();
    for mapping in mappings {
        by_path
            .entry(mapping.try_get("path")?)
            .or_default()
            .push(SnapshotChunk {
                digest: mapping.try_get("digest")?,
                plaintext_size: u64::try_from(mapping.try_get::<i64, _>("plaintext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                ciphertext_size: u64::try_from(mapping.try_get::<i64, _>("ciphertext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
            });
    }
    let files = upload_files
        .into_iter()
        .map(|file| SnapshotFile {
            chunks: by_path.remove(&file.path).unwrap_or_default(),
            path: file.path,
            size: file.size,
            mode: file.mode,
            modified_at: file.modified_at,
        })
        .collect();
    Ok(SnapshotManifest {
        schema_version: SCHEMA_VERSION,
        id: manifest_id,
        project_id: row.try_get("project_id")?,
        total_bytes: u64::try_from(row.try_get::<i64, _>("total_bytes")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        files,
        encryption_key_id: row.try_get("encryption_key_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

fn row_to_project_version(row: &sqlx::postgres::PgRow) -> Result<ProjectVersion, ApiError> {
    Ok(ProjectVersion {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        revision: row.try_get("revision")?,
        parent_version_id: row.try_get("parent_version_id")?,
        merge_base_version_id: row.try_get("merge_base_version_id")?,
        snapshot_manifest_id: row.try_get("snapshot_manifest_id")?,
        created_by_run_id: row.try_get("created_by_run_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn detect_secret_warnings(bytes: &[u8]) -> Vec<String> {
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

fn require_store(state: &AppState) -> Result<Arc<ObjectStore>, ApiError> {
    state.object_store.clone().ok_or_else(|| {
        ApiError::Conflict("object storage is not configured on this server".to_owned())
    })
}

fn decode_master_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let trimmed = value.trim();
    let decoded = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .or_else(|_| hex::decode(trimmed))?;
    anyhow::ensure!(
        decoded.len() == 32,
        "storage master key must decode to exactly 32 bytes"
    );
    let mut key = [0_u8; 32];
    key.copy_from_slice(&decoded);
    Ok(key)
}

fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, AWS_ENCODE_SET).to_string()
}

fn valid_virtual_host_bucket(value: &str) -> bool {
    if !(3..=63).contains(&value.len()) || value.parse::<IpAddr>().is_ok() || value.contains("..") {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
    })
}

fn hmac_bytes(key: &[u8], value: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid HMAC key")))?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn aws_signing_key(secret: &str, date: &str, region: &str) -> Result<Vec<u8>, ApiError> {
    let date_key = hmac_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let region_key = hmac_bytes(&date_key, region.as_bytes())?;
    let service_key = hmac_bytes(&region_key, b"s3")?;
    hmac_bytes(&service_key, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, extract::Request, http::HeaderMap, routing::any, Router};
    use std::{env, net::SocketAddr, sync::Mutex};
    use tokio::net::TcpListener;

    #[test]
    fn envelope_roundtrip_and_scope_separation() {
        let store = ObjectStore {
            s3: S3Client {
                http: Client::new(),
                endpoint: Url::parse("http://127.0.0.1:9000").unwrap(),
                region: "us-east-1".to_owned(),
                bucket: "test".to_owned(),
                addressing_style: S3AddressingStyle::Path,
                access_key: "test".to_owned(),
                secret_key: "test".to_owned(),
                session_token: None,
            },
            master_key: [7; 32],
        };
        let scope = KeyScope::User(Uuid::new_v4());
        let encrypted = store.encrypt(scope, b"classified").unwrap();
        let plaintext = store
            .decrypt(
                scope,
                &encrypted.ciphertext,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .unwrap();
        assert_eq!(plaintext, b"classified");
        assert!(store
            .decrypt(
                KeyScope::User(Uuid::new_v4()),
                &encrypted.ciphertext,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .is_err());
    }

    #[test]
    fn rejects_parent_and_windows_paths() {
        assert!(validate_relative_path("src/main.rs").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("C:\\secret").is_err());
    }

    #[test]
    fn accepts_only_single_safe_attachment_names() {
        assert!(validate_attachment_name("camera-photo.jpg").is_ok());
        assert!(validate_attachment_name("Bericht 2026.xlsx").is_ok());
        assert!(validate_attachment_name("../secret.txt").is_err());
        assert!(validate_attachment_name("folder/file.txt").is_err());
        assert!(validate_attachment_name("folder\\file.txt").is_err());
        assert!(validate_attachment_name("bad\0name.txt").is_err());
    }

    #[test]
    fn signing_key_is_deterministic_and_region_scoped() {
        let key = aws_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
        )
        .unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(
            key,
            aws_signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20120215",
                "us-east-1"
            )
            .unwrap()
        );
        assert_ne!(
            key,
            aws_signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20120215",
                "eu-central-1"
            )
            .unwrap()
        );
    }

    #[test]
    fn builds_path_and_virtual_hosted_locations() {
        let path_client = S3Client {
            http: Client::new(),
            endpoint: Url::parse("https://s3.eu-central-1.amazonaws.com").unwrap(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: S3AddressingStyle::Path,
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            session_token: None,
        };
        let (url, canonical, host) = path_client
            .request_location("chunks/ab/object name")
            .unwrap();
        assert_eq!(canonical, "/cowork-blobs/chunks/ab/object%20name");
        assert_eq!(
            url.as_str(),
            "https://s3.eu-central-1.amazonaws.com/cowork-blobs/chunks/ab/object%20name"
        );
        assert_eq!(host, "s3.eu-central-1.amazonaws.com");

        let virtual_client = S3Client {
            addressing_style: S3AddressingStyle::VirtualHosted,
            ..path_client
        };
        let (url, canonical, host) = virtual_client.request_location("chunks/ab/object").unwrap();
        assert_eq!(canonical, "/chunks/ab/object");
        assert_eq!(
            url.as_str(),
            "https://cowork-blobs.s3.eu-central-1.amazonaws.com/chunks/ab/object"
        );
        assert_eq!(host, "cowork-blobs.s3.eu-central-1.amazonaws.com");
    }

    #[test]
    fn validates_virtual_hosted_storage_configuration() {
        let base = ObjectStoreConfig {
            endpoint: "https://s3.eu-central-1.amazonaws.com".to_owned(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: "virtual_hosted".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            session_token: Some("temporary-session-token".to_owned()),
            master_key: STANDARD.encode([7_u8; 32]),
        };
        assert!(ObjectStore::from_config(&base).is_ok());

        let invalid_ip = ObjectStoreConfig {
            endpoint: "http://127.0.0.1:9000".to_owned(),
            ..base.clone()
        };
        assert!(ObjectStore::from_config(&invalid_ip).is_err());
        let invalid_bucket = ObjectStoreConfig {
            bucket: "Not_DNS_Compatible".to_owned(),
            ..base
        };
        assert!(ObjectStore::from_config(&invalid_bucket).is_err());
    }

    #[tokio::test]
    async fn sends_virtual_hosted_request_with_signed_session_token() {
        #[derive(Clone)]
        struct CapturedRequest {
            path: String,
            headers: HeaderMap,
            body: Vec<u8>,
        }

        let captured = Arc::new(Mutex::new(None::<CapturedRequest>));
        let capture_target = captured.clone();
        let app = Router::new().fallback(any(move |request: Request| {
            let capture_target = capture_target.clone();
            async move {
                let path = request.uri().path().to_owned();
                let headers = request.headers().clone();
                let body = to_bytes(request.into_body(), 1024).await.unwrap().to_vec();
                *capture_target.lock().unwrap() = Some(CapturedRequest {
                    path,
                    headers,
                    body,
                });
                StatusCode::OK
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let virtual_host = "cowork-blobs.s3.localhost";
        let client = S3Client {
            http: Client::builder()
                .resolve(
                    virtual_host,
                    SocketAddr::from(([127, 0, 0, 1], address.port())),
                )
                .build()
                .unwrap(),
            endpoint: Url::parse(&format!("http://s3.localhost:{}", address.port())).unwrap(),
            region: "eu-central-1".to_owned(),
            bucket: "cowork-blobs".to_owned(),
            addressing_style: S3AddressingStyle::VirtualHosted,
            access_key: "temporary-access".to_owned(),
            secret_key: "temporary-secret".to_owned(),
            session_token: Some("temporary-session-token".to_owned()),
        };

        client
            .request(Method::PUT, "chunks/ab/object", Some(b"payload".to_vec()))
            .await
            .unwrap();
        let captured = captured.lock().unwrap().clone().unwrap();
        assert_eq!(captured.path, "/chunks/ab/object");
        assert_eq!(captured.body, b"payload");
        assert_eq!(
            captured.headers.get("host").unwrap(),
            format!("{virtual_host}:{}", address.port()).as_str()
        );
        assert_eq!(
            captured.headers.get("x-amz-security-token").unwrap(),
            "temporary-session-token"
        );
        let authorization = captured
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(authorization
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires explicit COWORK_S3_TEST_* credentials and mutates only the configured test bucket"]
    async fn external_s3_provider_acceptance() {
        let required = |name: &str| {
            env::var(name).unwrap_or_else(|_| panic!("missing acceptance-test variable {name}"))
        };
        let config = ObjectStoreConfig {
            endpoint: required("COWORK_S3_TEST_ENDPOINT"),
            region: required("COWORK_S3_TEST_REGION"),
            bucket: required("COWORK_S3_TEST_BUCKET"),
            addressing_style: env::var("COWORK_S3_TEST_ADDRESSING_STYLE")
                .unwrap_or_else(|_| "path".to_owned()),
            access_key: required("COWORK_S3_TEST_ACCESS_KEY"),
            secret_key: required("COWORK_S3_TEST_SECRET_KEY"),
            session_token: env::var("COWORK_S3_TEST_SESSION_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            master_key: STANDARD.encode([41_u8; 32]),
        };
        let store = ObjectStore::from_config(&config).unwrap();
        let scope = KeyScope::User(Uuid::new_v4());
        let plaintext = format!("open-cowork-s3-acceptance-{}", Uuid::new_v4()).into_bytes();
        let digest = hex::encode(Sha256::digest(&plaintext));
        let (object_key, encrypted) = store
            .put_encrypted(scope, &digest, &plaintext)
            .await
            .unwrap();
        let roundtrip = store
            .get_decrypted(
                scope,
                &object_key,
                &encrypted.wrapped_data_key,
                &encrypted.nonce,
                &encrypted.wrap_nonce,
            )
            .await
            .unwrap();
        assert_eq!(roundtrip, plaintext);
        store.delete(&object_key).await.unwrap();
    }
}
