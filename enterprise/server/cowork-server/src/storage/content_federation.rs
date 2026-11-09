//! Copies actual immutable snapshot files into a new owned project. No sessions,
//! model profiles, executor grants, project policies or runnable definitions are imported.
use super::{
    envelope::{KeyScope, SealedValue},
    object_store::require_store,
    snapshot_repository as repo,
};
use crate::{
    auth::Principal,
    error::ApiError,
    management::content_federation::{Callback, Proof},
    AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use cowork_contracts::{ProjectRole, SnapshotFile, SnapshotUploadFile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    path: String,
    size: u64,
    mode: u32,
    modified_at: DateTime<Utc>,
    sha256: String,
    bytes: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    format: u16,
    transfer_id: Uuid,
    proof: Proof,
    original_owner_user_id: Uuid,
    project_name: String,
    project_description: String,
    source_snapshot_manifest_id: Uuid,
    files: Vec<File>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportQuery {
    grant_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    transfer_id: Uuid,
    grant_id: Uuid,
    bundle: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validate {
    transfer_id: Uuid,
    grant_id: Uuid,
    bundle_sha256: String,
}
struct VerifiedRegistry(Proof); // Constructed only by this instance's actual configured fleet callback.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/versions/{version}/content-sharing/{transfer}",
            get(export),
        )
        .route(
            "/enterprise/content-sharing/imports",
            post(import).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/enterprise/content-sharing/imports/{transfer}",
            get(inspect),
        )
}
pub fn proof_router() -> Router<AppState> {
    Router::new().route("/content-sharing/validate", post(validate))
}
fn denied() -> ApiError {
    ApiError::Unauthorized("current original content-sharing authority is unavailable".into())
}
fn conflict() -> ApiError {
    ApiError::Conflict("original content transfer or immutable bytes differ".into())
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn scope() -> Result<(Uuid, Uuid), ApiError> {
    let parse = |key| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<Uuid>().ok())
            .filter(|id| !id.is_nil())
            .ok_or_else(denied)
    };
    if !crate::fleet::enabled() {
        return Err(denied());
    }
    Ok((
        parse("COWORK_ORGANISATION_ID")?,
        parse("COWORK_INSTANCE_ID")?,
    ))
}
async fn actor(state: &AppState, user: Uuid, sid: Uuid) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    crate::organisation_mfa::session_in_tx(state, &mut tx, user, sid, false).await?;
    crate::organisation_mfa::session_tx(&mut tx, user, Some(sid)).await?;
    tx.commit().await?;
    Ok(())
}
async fn callback(
    state: &AppState,
    user: Uuid,
    grant: Uuid,
    transfer: Uuid,
    action: &str,
    sha: Option<&str>,
    receipt: Option<Value>,
) -> Result<Value, ApiError> {
    let (org, instance) = scope()?;
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| denied())?;
    let client = crate::management_transport::client(&base, std::time::Duration::from_secs(20))?;
    let key = crate::fleet::secret("COWORK_FLEET_KEY_FILE")?;
    let q = Callback {
        organisation_id: org,
        instance_id: instance,
        user_id: user,
        grant_id: grant,
        transfer_id: transfer,
        action: action.into(),
        bundle_sha256: sha.map(str::to_owned),
        receipt,
    };
    let endpoint = if matches!(action, "publish" | "commit") {
        "receipt"
    } else {
        "authorize"
    };
    let mut response = client
        .post(format!(
            "{}/management/runner/v1/content-sharing/{endpoint}",
            base.trim_end_matches('/')
        ))
        .header("x-cowork-instance-key", &key)
        .json(&q)
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
    {
        if bytes.len() + chunk.len() > 128 * 1024 {
            return Err(denied());
        }
        bytes.extend_from_slice(&chunk)
    }
    if crate::fleet::secret("COWORK_FLEET_KEY_FILE")? != key {
        return Err(denied());
    }
    let _ = state;
    Ok(serde_json::from_slice(&bytes)?)
}
async fn registry(
    state: &AppState,
    user: Uuid,
    grant: Uuid,
    transfer: Uuid,
    source: bool,
    sha: Option<&str>,
) -> Result<VerifiedRegistry, ApiError> {
    let p: Proof = serde_json::from_value(
        callback(
            state,
            user,
            grant,
            transfer,
            if source { "export" } else { "import" },
            sha,
            None,
        )
        .await?,
    )?;
    let (org, instance) = scope()?;
    let valid = if source {
        p.source_organisation_id == org
            && p.source_instance_id == instance
            && p.publisher_user_id == user
    } else {
        p.target_organisation_id == org
            && p.target_instance_id == instance
            && p.recipient_user_id == user
    };
    if !valid
        || p.grant_id != grant
        || p.revision < 2
        || p.epoch.is_nil()
        || p.expires_at <= Utc::now()
        || p.paths.is_empty()
        || p.paths.len() > 64
    {
        return Err(denied());
    }
    Ok(VerifiedRegistry(p))
}
fn same(a: &Proof, b: &Proof) -> Result<(), ApiError> {
    if serde_json::to_vec(a)? != serde_json::to_vec(b)? {
        return Err(denied());
    }
    Ok(())
}
async fn source_object(
    state: &AppState,
    user: Uuid,
    sid: Uuid,
    project: Uuid,
    version: Uuid,
) -> Result<(sqlx::postgres::PgRow, sqlx::postgres::PgRow), ApiError> {
    actor(state, user, sid).await?;
    crate::organization::ensure_project_role(&state.pool, user, project, ProjectRole::Viewer)
        .await?;
    let project_row=sqlx::query("SELECT p.* FROM projects p JOIN users u ON u.id=p.owner_user_id AND u.deleted_at IS NULL WHERE p.id=$1 AND p.deleted_at IS NULL").bind(project).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    let info = crate::fleet_content::context(user, None).await?;
    if project_row.get::<Uuid, _>("owner_user_id") != user {
        if !info.support_administrator {
            return Err(denied());
        }
        if !info.automatic_content_access
            && crate::governance::active_project_support_grant(&state.pool, user, project)
                .await?
                .is_none()
        {
            return Err(denied());
        }
    }
    let manifest=sqlx::query("SELECT s.* FROM project_versions v JOIN snapshot_manifests s ON s.id=v.snapshot_manifest_id AND s.project_id=v.project_id WHERE v.id=$1 AND v.project_id=$2 AND s.status='ready'").bind(version).bind(project).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    actor(state, user, sid).await?;
    Ok((project_row, manifest))
}
fn safe(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    cowork_contracts::enterprise::safe_relative_path(path)
        && path.len() <= 1024
        && !lower.split('/').any(|p| {
            p == ".env"
                || p.starts_with(".env.")
                || matches!(p, ".git" | ".ssh" | ".aws" | "credentials" | "secrets")
                || p.ends_with(".pem")
                || p.ends_with(".key")
                || p.ends_with(".p12")
                || p.ends_with(".pfx")
        })
}
fn sealed(r: &sqlx::postgres::PgRow) -> Result<SealedValue, ApiError> {
    Ok(SealedValue {
        ciphertext: r.get("ciphertext"),
        encrypted_data_key: r.get("encrypted_data_key"),
        nonce: r
            .get::<Vec<u8>, _>("nonce")
            .try_into()
            .map_err(|_| conflict())?,
        wrap_nonce: r
            .get::<Vec<u8>, _>("wrap_nonce")
            .try_into()
            .map_err(|_| conflict())?,
    })
}
async fn export(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((project, version, transfer)): Path<(Uuid, Uuid, Uuid)>,
    Query(q): Query<ExportQuery>,
) -> Result<Json<Value>, ApiError> {
    if principal.bootstrap || transfer.is_nil() {
        return Err(denied());
    }
    let sid = principal.session_id.ok_or_else(denied)?;
    let verified = registry(&state, principal.user_id, q.grant_id, transfer, true, None).await?;
    let p = &verified.0;
    if p.project_id != project || p.version_id != version {
        return Err(denied());
    }
    let (project_row, manifest) =
        source_object(&state, principal.user_id, sid, project, version).await?;
    let key = state.auth_secrets.as_ref().ok_or_else(denied)?;
    let existing = sqlx::query("SELECT * FROM instance_content_exports WHERE transfer_id=$1")
        .bind(transfer)
        .fetch_optional(&state.pool)
        .await?;
    let (bytes, receipt) = if let Some(row) = existing {
        if row.get::<Uuid, _>("grant_id") != p.grant_id
            || row.get::<i64, _>("grant_revision") != p.revision
            || row.get::<Uuid, _>("grant_epoch") != p.epoch
            || row.get::<Uuid, _>("publisher_user_id") != principal.user_id
            || row.get::<Uuid, _>("publisher_session_id") != sid
            || row.get::<Uuid, _>("original_owner_user_id")
                != project_row.get::<Uuid, _>("owner_user_id")
        {
            return Err(conflict());
        }
        let bytes = key.open_for_user(principal.user_id, &sealed(&row)?)?;
        if hash(&bytes) != row.get::<String, _>("bundle_sha256") {
            return Err(conflict());
        }
        (bytes, row.get::<Value, _>("source_receipt"))
    } else {
        let store = require_store(&state)?;
        let files: Vec<SnapshotFile> = serde_json::from_value(manifest.get("manifest"))?;
        let mut selected = Vec::new();
        let mut total = 0usize;
        for path in &p.paths {
            if !safe(path) {
                return Err(ApiError::Unprocessable(
                    "credential/control paths cannot be shared as documents".into(),
                ));
            }
            let f = files.iter().find(|f| &f.path == path).ok_or_else(denied)?;
            if f.size > 16 * 1024 * 1024 {
                return Err(conflict());
            }
            let mut bytes = Vec::new();
            for chunk in &f.chunks {
                let part = repo::decrypt_manifest_chunk(
                    &state.pool,
                    &store,
                    &manifest,
                    manifest.get("id"),
                    &chunk.digest,
                )
                .await?;
                if part.len() as u64 != chunk.plaintext_size
                    || bytes.len() + part.len() > 16 * 1024 * 1024
                {
                    return Err(conflict());
                }
                bytes.extend_from_slice(&part);
                source_object(&state, principal.user_id, sid, project, version).await?;
            }
            if bytes.len() as u64 != f.size {
                return Err(conflict());
            }
            total = total.checked_add(bytes.len()).ok_or_else(conflict)?;
            if total > 16 * 1024 * 1024 {
                return Err(ApiError::Unprocessable(
                    "content copy exceeds the 16 MiB plaintext bound".into(),
                ));
            }
            selected.push(File {
                path: f.path.clone(),
                size: f.size,
                mode: f.mode,
                modified_at: f.modified_at,
                sha256: hash(&bytes),
                bytes: BASE64.encode(bytes),
            });
        }
        let bundle = Bundle {
            format: 1,
            transfer_id: transfer,
            proof: p.clone(),
            original_owner_user_id: project_row.get("owner_user_id"),
            project_name: project_row.get("name"),
            project_description: project_row.get("description"),
            source_snapshot_manifest_id: manifest.get("id"),
            files: selected,
        };
        let bytes = serde_json::to_vec(&bundle)?;
        if bytes.len() > 24 * 1024 * 1024 {
            return Err(conflict());
        }
        let digest = hash(&bytes);
        let current = registry(&state, principal.user_id, p.grant_id, transfer, true, None).await?;
        same(p, &current.0)?;
        source_object(&state, principal.user_id, sid, project, version).await?;
        let sealed = key.seal_for_user(principal.user_id, &bytes)?;
        let receipt = json!({"format":1,"transfer_id":transfer,"grant_id":p.grant_id,"grant_revision":p.revision,"grant_epoch":p.epoch,"source_instance_id":p.source_instance_id,"source_epoch":p.source_epoch,"project_id":project,"version_id":version,"snapshot_manifest_id":bundle.source_snapshot_manifest_id,"original_owner_user_id":bundle.original_owner_user_id,"publisher_user_id":principal.user_id,"bundle_sha256":digest,"size":bytes.len(),"execution_authority_copied":false});
        let mut tx = state.pool.begin().await?;
        crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false)
            .await?;
        let owner: Uuid = sqlx::query_scalar(
            "SELECT owner_user_id FROM projects WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
        )
        .bind(project)
        .fetch_one(&mut *tx)
        .await?;
        if owner != bundle.original_owner_user_id {
            return Err(denied());
        }
        sqlx::query("INSERT INTO instance_content_exports(transfer_id,grant_id,grant_revision,grant_epoch,source_instance_id,source_epoch,publisher_user_id,publisher_session_id,project_id,version_id,original_owner_user_id,bundle_sha256,ciphertext,encrypted_data_key,nonce,wrap_nonce,source_receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)").bind(transfer).bind(p.grant_id).bind(p.revision).bind(p.epoch).bind(p.source_instance_id).bind(p.source_epoch).bind(principal.user_id).bind(sid).bind(project).bind(version).bind(owner).bind(&digest).bind(sealed.ciphertext).bind(sealed.encrypted_data_key).bind(sealed.nonce.as_slice()).bind(sealed.wrap_nonce.as_slice()).bind(&receipt).execute(&mut *tx).await?;
        tx.commit().await?;
        (bytes, receipt)
    };
    let current = registry(&state, principal.user_id, p.grant_id, transfer, true, None).await?;
    same(p, &current.0)?;
    source_object(&state, principal.user_id, sid, project, version).await?;
    let management = callback(
        &state,
        principal.user_id,
        p.grant_id,
        transfer,
        "publish",
        Some(&hash(&bytes)),
        Some(receipt.clone()),
    )
    .await?;
    if management["original_receipt"] != receipt {
        return Err(conflict());
    }
    source_object(&state, principal.user_id, sid, project, version).await?;
    sqlx::query("UPDATE instance_content_exports SET management_receipt=$2 WHERE transfer_id=$1 AND management_receipt IS NULL").bind(transfer).bind(&management).execute(&state.pool).await?;
    Ok(Json(
        json!({"format":1,"transfer_id":transfer,"grant_id":p.grant_id,"bundle_sha256":hash(&bytes),"source_receipt":receipt,"management_receipt":management,"bundle":BASE64.encode(bytes)}),
    ))
}
async fn validate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(q): Json<Validate>,
) -> Result<Json<Value>, ApiError> {
    use subtle::ConstantTimeEq;
    let key = crate::fleet::secret("COWORK_FLEET_KEY_FILE")?;
    let supplied = headers
        .get("x-cowork-instance-key")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(denied)?;
    if headers.get_all("x-cowork-instance-key").iter().count() != 1
        || !bool::from(key.as_bytes().ct_eq(supplied.as_bytes()))
    {
        return Err(denied());
    }
    let row=sqlx::query("SELECT * FROM instance_content_exports WHERE transfer_id=$1 AND grant_id=$2 AND bundle_sha256=$3 AND management_receipt IS NOT NULL").bind(q.transfer_id).bind(q.grant_id).bind(&q.bundle_sha256).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    let user: Uuid = row.get("publisher_user_id");
    let sid: Uuid = row.get("publisher_session_id");
    let p = registry(&state, user, q.grant_id, q.transfer_id, true, None).await?;
    if p.0.source_epoch != row.get::<Uuid, _>("source_epoch")
        || p.0.revision != row.get::<i64, _>("grant_revision")
        || p.0.epoch != row.get::<Uuid, _>("grant_epoch")
    {
        return Err(denied());
    }
    let (project, _) = source_object(
        &state,
        user,
        sid,
        row.get("project_id"),
        row.get("version_id"),
    )
    .await?;
    if project.get::<Uuid, _>("owner_user_id") != row.get::<Uuid, _>("original_owner_user_id")
        || crate::fleet::secret("COWORK_FLEET_KEY_FILE")? != key
    {
        return Err(denied());
    }
    Ok(Json(
        json!({"current_authorized":true,"transfer_id":q.transfer_id,"grant_id":q.grant_id,"bundle_sha256":q.bundle_sha256,"source_epoch":p.0.source_epoch}),
    ))
}
async fn target(state: &AppState, user: Uuid, sid: Uuid) -> Result<(), ApiError> {
    actor(state, user, sid).await?;
    let access = crate::fleet_content::context(user, None).await?;
    if access.read_only || !access.local_membership_allowed() {
        return Err(denied());
    }
    Ok(())
}
async fn import(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(q): Json<Import>,
) -> Result<Json<Value>, ApiError> {
    if principal.bootstrap || q.transfer_id.is_nil() || q.bundle.len() > 32 * 1024 * 1024 {
        return Err(denied());
    }
    let sid = principal.session_id.ok_or_else(denied)?;
    target(&state, principal.user_id, sid).await?;
    let bytes = BASE64.decode(&q.bundle).map_err(|_| conflict())?;
    if bytes.len() > 24 * 1024 * 1024 {
        return Err(conflict());
    }
    let digest = hash(&bytes);
    let bundle: Bundle = serde_json::from_slice(&bytes)?;
    if bundle.format != 1
        || bundle.transfer_id != q.transfer_id
        || bundle.proof.grant_id != q.grant_id
        || bundle.files.is_empty()
        || bundle.files.len() > 64
        || bundle.project_name.is_empty()
        || bundle.project_name.len() > 120
        || bundle.project_name.chars().any(char::is_control)
        || bundle.project_description.len() > 16384
    {
        return Err(conflict());
    }
    if let Some(old) = sqlx::query("SELECT * FROM instance_content_imports WHERE transfer_id=$1")
        .bind(q.transfer_id)
        .fetch_optional(&state.pool)
        .await?
    {
        if old.get::<Uuid, _>("target_instance_id") != scope()?.1
            || old.get::<Uuid, _>("recipient_user_id") != principal.user_id
            || old.get::<String, _>("bundle_sha256") != digest
            || old.get::<Uuid, _>("grant_id") != q.grant_id
        {
            return Err(conflict());
        }
        if old.get::<String, _>("phase") == "committed" {
            return Ok(Json(old.get("local_receipt")));
        }
    }
    let verified = registry(
        &state,
        principal.user_id,
        q.grant_id,
        q.transfer_id,
        false,
        Some(&digest),
    )
    .await?;
    let p = &verified.0;
    same(p, &bundle.proof)?;
    let mut decoded = Vec::new();
    let mut total = 0usize;
    let mut paths = std::collections::HashSet::new();
    for f in &bundle.files {
        if !safe(&f.path) || !p.paths.contains(&f.path) || !paths.insert(f.path.clone()) {
            return Err(conflict());
        }
        let bytes = BASE64.decode(&f.bytes).map_err(|_| conflict())?;
        if bytes.len() as u64 != f.size || hash(&bytes) != f.sha256 {
            return Err(conflict());
        }
        total = total.checked_add(bytes.len()).ok_or_else(conflict)?;
        if total > 16 * 1024 * 1024 {
            return Err(conflict());
        }
        decoded.push(bytes)
    }
    if paths.len() != p.paths.len() {
        return Err(conflict());
    }
    let key = state.auth_secrets.as_ref().ok_or_else(denied)?;
    let project_id = {
        let mut tx = state.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("content-import:{}", q.transfer_id))
            .execute(&mut *tx)
            .await?;
        crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false)
            .await?;
        let old =
            sqlx::query("SELECT * FROM instance_content_imports WHERE transfer_id=$1 FOR UPDATE")
                .bind(q.transfer_id)
                .fetch_optional(&mut *tx)
                .await?;
        let id = if let Some(old) = old {
            if old.get::<Uuid, _>("target_instance_id") != scope()?.1
                || old.get::<Uuid, _>("recipient_user_id") != principal.user_id
                || old.get::<Uuid, _>("recipient_session_id") != sid
                || old.get::<String, _>("bundle_sha256") != digest
                || old.get::<i64, _>("grant_revision") != p.revision
                || old.get::<Uuid, _>("grant_epoch") != p.epoch
            {
                return Err(conflict());
            }
            old.get("project_id")
        } else {
            crate::governance::enforce_storage_quota_tx(
                &mut tx,
                "user",
                principal.user_id,
                total as u64,
            )
            .await?;
            let id = Uuid::new_v4();
            let sealed = key.seal_for_user(principal.user_id, &bytes)?;
            sqlx::query("INSERT INTO projects(id,etag,owner_user_id,privacy,name,description,policy,deleted_at) VALUES($1,$2,$3,'private_local',$4,$5,'{}'::jsonb,clock_timestamp())").bind(id).bind(format!("W/\"{id}:1\"")).bind(principal.user_id).bind(&bundle.project_name).bind(&bundle.project_description).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO instance_content_imports(transfer_id,grant_id,grant_revision,grant_epoch,source_instance_id,target_instance_id,target_epoch,recipient_user_id,recipient_session_id,bundle_sha256,phase,project_id,ciphertext,encrypted_data_key,nonce,wrap_nonce) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'prepared',$11,$12,$13,$14,$15)").bind(q.transfer_id).bind(p.grant_id).bind(p.revision).bind(p.epoch).bind(p.source_instance_id).bind(p.target_instance_id).bind(p.target_epoch).bind(principal.user_id).bind(sid).bind(&digest).bind(id).bind(sealed.ciphertext).bind(sealed.encrypted_data_key).bind(sealed.nonce.as_slice()).bind(sealed.wrap_nonce.as_slice()).execute(&mut *tx).await?;
            id
        };
        tx.commit().await?;
        id
    };
    let store = require_store(&state)?;
    let scope = KeyScope::User(principal.user_id);
    let old = sqlx::query("SELECT * FROM instance_content_imports WHERE transfer_id=$1")
        .bind(q.transfer_id)
        .fetch_one(&state.pool)
        .await?;
    let receipt = if old.get::<String, _>("phase") == "staged" {
        old.get::<Value, _>("local_receipt")
    } else {
        let mut files = Vec::new();
        for (f, bytes) in bundle.files.iter().zip(&decoded) {
            let template = SnapshotFile {
                path: f.path.clone(),
                size: f.size,
                mode: f.mode & 0o666,
                modified_at: f.modified_at,
                chunks: Vec::new(),
            };
            files.push(
                repo::upload_file_from_bytes(&state.pool, &store, scope, &template, bytes).await?,
            );
            target(&state, principal.user_id, sid).await?;
            let current = registry(
                &state,
                principal.user_id,
                p.grant_id,
                q.transfer_id,
                false,
                Some(&digest),
            )
            .await?;
            same(p, &current.0)?;
        }
        let templates = files
            .into_iter()
            .map(repo::snapshot_to_upload_file)
            .collect::<Vec<SnapshotUploadFile>>();
        let manifest_id = repo::create_ready_manifest(
            &state.pool,
            project_id,
            principal.user_id,
            scope,
            &templates,
        )
        .await?;
        target(&state, principal.user_id, sid).await?;
        let mut tx = state.pool.begin().await?;
        sqlx::query("SELECT transfer_id FROM instance_content_imports WHERE transfer_id=$1 AND phase='prepared' FOR UPDATE").bind(q.transfer_id).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
        crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false)
            .await?;
        let version_id = Uuid::new_v4();
        sqlx::query("INSERT INTO project_versions(id,project_id,revision,snapshot_manifest_id,created_by_user_id,diff_summary) VALUES($1,$2,1,$3,$4,$5)").bind(version_id).bind(project_id).bind(manifest_id).bind(principal.user_id).bind(json!({"content_transfer_id":q.transfer_id,"source_project_id":p.project_id,"source_version_id":p.version_id,"source_instance_id":p.source_instance_id,"execution_authority_copied":false})).execute(&mut *tx).await?;
        sqlx::query("UPDATE projects SET current_version_id=$2 WHERE id=$1 AND owner_user_id=$3 AND deleted_at IS NOT NULL").bind(project_id).bind(version_id).bind(principal.user_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE snapshot_manifests SET expires_at=NULL WHERE id=$1 AND project_id=$2 AND status='ready'").bind(manifest_id).bind(project_id).execute(&mut *tx).await?;
        let receipt = json!({"format":1,"transfer_id":q.transfer_id,"grant_id":p.grant_id,"grant_revision":p.revision,"grant_epoch":p.epoch,"bundle_sha256":digest,"source_instance_id":p.source_instance_id,"target_instance_id":p.target_instance_id,"target_epoch":p.target_epoch,"recipient_user_id":principal.user_id,"project_id":project_id,"version_id":version_id,"snapshot_manifest_id":manifest_id,"source_project_id":p.project_id,"source_version_id":p.version_id,"file_count":bundle.files.len(),"plaintext_bytes":total,"execution_authority_copied":false});
        sqlx::query("UPDATE instance_content_imports SET phase='staged',version_id=$2,snapshot_manifest_id=$3,local_receipt=$4 WHERE transfer_id=$1 AND phase='prepared'").bind(q.transfer_id).bind(version_id).bind(manifest_id).bind(&receipt).execute(&mut *tx).await?;
        tx.commit().await?;
        receipt
    };
    let current = registry(
        &state,
        principal.user_id,
        p.grant_id,
        q.transfer_id,
        false,
        Some(&digest),
    )
    .await?;
    same(p, &current.0)?;
    target(&state, principal.user_id, sid).await?;
    let management = callback(
        &state,
        principal.user_id,
        p.grant_id,
        q.transfer_id,
        "commit",
        Some(&digest),
        Some(receipt.clone()),
    )
    .await?;
    if management["original_receipt"] != receipt {
        return Err(conflict());
    }
    let current = registry(
        &state,
        principal.user_id,
        p.grant_id,
        q.transfer_id,
        false,
        Some(&digest),
    )
    .await?;
    same(p, &current.0)?;
    target(&state, principal.user_id, sid).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT transfer_id FROM instance_content_imports WHERE transfer_id=$1 AND phase='staged' FOR UPDATE").bind(q.transfer_id).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false).await?;
    let current = registry(
        &state,
        principal.user_id,
        p.grant_id,
        q.transfer_id,
        false,
        Some(&digest),
    )
    .await?;
    same(p, &current.0)?;
    crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false).await?;
    crate::organisation_mfa::session_tx(&mut tx, principal.user_id, Some(sid)).await?;
    let unexpired: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(p.expires_at)
        .fetch_one(&mut *tx)
        .await?;
    if !unexpired {
        return Err(denied());
    }
    sqlx::query("UPDATE projects SET deleted_at=NULL WHERE id=$1 AND owner_user_id=$2 AND current_version_id=$3").bind(project_id).bind(principal.user_id).bind(serde_json::from_value::<Uuid>(receipt["version_id"].clone())?).execute(&mut *tx).await?;
    sqlx::query("UPDATE instance_content_imports SET phase='committed',management_receipt=$2 WHERE transfer_id=$1 AND phase='staged'").bind(q.transfer_id).bind(&management).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn inspect(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    actor(&state, p.user_id, p.session_id.ok_or_else(denied)?).await?;
    let row=sqlx::query("SELECT phase,local_receipt,management_receipt,bundle_sha256,project_id FROM instance_content_imports WHERE transfer_id=$1 AND recipient_user_id=$2 AND target_instance_id=$3").bind(id).bind(p.user_id).bind(scope()?.1).fetch_optional(&state.pool).await?.ok_or_else(denied)?;
    Ok(Json(
        json!({"format":1,"transfer_id":id,"phase":row.get::<String,_>("phase"),"project_id":row.get::<Uuid,_>("project_id"),"bundle_sha256":row.get::<String,_>("bundle_sha256"),"local_receipt":row.get::<Option<Value>,_>("local_receipt"),"management_receipt":row.get::<Option<Value>,_>("management_receipt")}),
    ))
}
