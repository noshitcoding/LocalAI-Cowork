//! Original checkpoint bytes and explicit same-run private server continuation.
//! SQL locators carry no runtime grant; encrypted system envelopes retain proof.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use crate::android_private_server::{id, row, save};
use crate::android_private_workspace as workspace;
use cowork_contracts::{
    AndroidResumeRun, AndroidRunCheckpoint, AndroidRunCheckpointDescriptor,
    AndroidRunCheckpointPage, AndroidRunResumeReceipt, AndroidRunWaiting, AndroidRunWaitingReason,
};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;
use zeroize::Zeroizing;

const PAGE: usize = 1024 * 1024;
const SYSTEM: &str = ".cowork-system/run-recovery/v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatePart {
    id: String,
    sha256: String,
    size: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointHeader {
    format: String,
    owner_id: Uuid,
    project_id: Uuid,
    thread_id: Uuid,
    checkpoint_sha256: String,
    source_revision: i64,
    spec_sha256: String,
    original_lease_id: Option<String>,
    descriptor: AndroidRunCheckpointDescriptor,
    parts: Vec<StatePart>,
}
fn system_id(purpose: &str, binding: &Value) -> Result<String> {
    let sha = canonical_hash(&json!((SYSTEM, purpose, binding)));
    Ok(Uuid::parse_str(&sha[..32])
        .map_err(storage_error)?
        .to_string())
}
async fn system_version(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    version: &str,
    purpose: &str,
    bytes: &[u8],
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<()> {
    let path = format!("{SYSTEM}/{purpose}/{version}");
    if let Some(old) = sqlx::query(
        "SELECT run_id,path,kind,digest,size FROM android_private_loop_versions WHERE id=$1",
    )
    .bind(version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    {
        if old.try_get::<String, _>("run_id").map_err(storage_error)? != id(r, "id")?.to_string()
            || old.try_get::<String, _>("path").map_err(storage_error)? != path
            || old.try_get::<String, _>("kind").map_err(storage_error)? != "artifact"
            || old.try_get::<String, _>("digest").map_err(storage_error)? != hash(bytes)
            || old.try_get::<i64, _>("size").map_err(storage_error)? != bytes.len() as i64
        {
            return Err(Error::Conflict);
        }
        // Even an identical locator must open its original authenticated bytes.
        if workspace::version_bytes(tx, r, version, keys)
            .await?
            .as_slice()
            != bytes
        {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    let sealed = workspace::seal_value(keys, key_id, r, &format!("file-version:{version}"), bytes)?;
    crate::server_keys::cipher_writer(tx, key_id).await?;
    sqlx::query("INSERT INTO android_private_loop_versions(id,run_id,path,kind,digest,size,sealed,created_at) VALUES($1,$2,$3,'artifact',$4,$5,$6,$7)")
        .bind(version).bind(id(r,"id")?.to_string()).bind(path).bind(hash(bytes)).bind(bytes.len() as i64).bind(json_string(&sealed)?).bind(current)
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn open_system(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    version: &str,
    purpose: &str,
    keys: &dyn KeyStore,
) -> Result<Zeroizing<Vec<u8>>> {
    let path: Option<(String, String)> = sqlx::query_as(
        "SELECT path,kind FROM android_private_loop_versions WHERE id=$1 AND run_id=$2",
    )
    .bind(version)
    .bind(id(r, "id")?.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    if path != Some((format!("{SYSTEM}/{purpose}/{version}"), "artifact".into())) {
        return Err(Error::Forbidden);
    }
    workspace::version_bytes(tx, r, version, keys).await
}
fn payload(r: &AnyRow) -> Result<Value> {
    parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)
}
fn revision(r: &AnyRow) -> Result<i64> {
    payload(r)?["revision"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or(Error::Conflict)
}
async fn private_actor(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    r: &AnyRow,
    action: ContentAction,
) -> Result<()> {
    store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    if id(r, "owner_id")? != actor.identity_id() {
        return Err(Error::Forbidden);
    }
    store
        .android_personal_permit(tx, actor, id(r, "project_id")?, action)
        .await?;
    Ok(())
}

/// Called only by the trusted checkpoint CAS writer in that same transaction.
pub(crate) async fn capture_checkpoint(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<()> {
    let source = sqlx::query(
        "SELECT digest,revision,updated_at FROM android_private_loop_checkpoints WHERE run_id=$1",
    )
    .bind(id(r, "id")?.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let digest: String = source.try_get("digest").map_err(storage_error)?;
    if digest != canonical_hash(cp) {
        return Err(Error::Conflict);
    }
    let sequence: i64 = source.try_get("revision").map_err(storage_error)?;
    let state = Zeroizing::new(serde_json::to_vec(cp).map_err(storage_error)?);
    if state.len() > 16 * PAGE {
        return Err(Error::BudgetExceeded);
    }
    let run = id(r, "id")?;
    if let Some(index)=sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE run_id=$1 AND owner_id=$2 AND origin='private_server' AND sequence=$3")
        .bind(run.to_string()).bind(id(r,"owner_id")?.to_string()).bind(sequence).fetch_optional(&mut **tx).await.map_err(storage_error)? {
        let original=validate_header(tx,r,&index,keys).await?;
        if original.checkpoint_sha256!=digest||original.descriptor.state_sha256!=hash(&state)||original.descriptor.state_size!=state.len() as u64 {return Err(Error::Conflict)}
        return Ok(());
    }
    let binding = json!((id(r, "owner_id")?, run, sequence, &digest));
    let checkpoint_id = system_id("checkpoint", &binding)?;
    let mut parts = Vec::new();
    for (page, bytes) in state.chunks(PAGE).enumerate() {
        let version = system_id("checkpoint-state", &json!((&binding, page, hash(bytes))))?;
        system_version(
            tx,
            r,
            &version,
            "checkpoint-state",
            bytes,
            keys,
            key_id,
            current,
        )
        .await?;
        parts.push(StatePart {
            id: version,
            sha256: hash(bytes),
            size: bytes.len() as u64,
        });
    }
    let spec_sha = canonical_hash(&payload(r)?["spec"]);
    // This server-owned diagnostic never trusts an agent's supplied Boolean.
    let safe = progress(tx, r, cp, keys).await.is_ok();
    let header = CheckpointHeader {
        format: "android-private-server-checkpoint/1".into(),
        owner_id: id(r, "owner_id")?,
        project_id: id(r, "project_id")?,
        thread_id: id(r, "thread_id")?,
        checkpoint_sha256: digest.clone(),
        source_revision: sequence,
        spec_sha256: spec_sha.clone(),
        original_lease_id: r.try_get("lease_id").map_err(storage_error)?,
        descriptor: AndroidRunCheckpointDescriptor {
            id: Uuid::parse_str(&checkpoint_id).map_err(storage_error)?,
            run_id: run,
            sequence,
            origin: "private_server".into(),
            safe_to_resume: safe,
            state_sha256: hash(&state),
            state_size: state.len() as u64,
            state_pages: parts.len() as u32,
            state_page_sha256: parts.iter().map(|p| p.sha256.clone()).collect(),
            created_at: device_iso(source.try_get("updated_at").map_err(storage_error)?)?,
        },
        parts,
    };
    let raw = Zeroizing::new(serde_json::to_vec(&header).map_err(storage_error)?);
    let header_id = system_id("checkpoint-header", &binding)?;
    system_version(
        tx,
        r,
        &header_id,
        "checkpoint-header",
        &raw,
        keys,
        key_id,
        current,
    )
    .await?;
    let n=sqlx::query("INSERT INTO android_run_checkpoint_index(id,run_id,owner_id,origin,sequence,source_revision,checkpoint_sha256,original_payload_sha256,spec_sha256,original_lease_id,header_version_id,state_version_id,state_sha256,state_size,safe_to_resume,created_at,source_table,source_row_id) VALUES($1,$2,$3,'private_server',$4,$4,$5,$6,$7,$8,$9,$9,$10,$11,$12,$13,'android_private_loop_checkpoints',$2) ON CONFLICT DO NOTHING")
        .bind(&checkpoint_id).bind(run.to_string()).bind(header.owner_id.to_string()).bind(sequence).bind(digest).bind(hash(&raw)).bind(spec_sha).bind(&header.original_lease_id).bind(&header_id)
        .bind(&header.descriptor.state_sha256).bind(state.len() as i64).bind(i64::from(safe)).bind(source.try_get::<i64,_>("updated_at").map_err(storage_error)?)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    if n == 0 {
        let original = sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE id=$1")
            .bind(checkpoint_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        validate_header(tx, r, &original, keys).await?;
    }
    workspace::bounds(tx, run).await
}
async fn validate_header(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    index: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<CheckpointHeader> {
    let version: String = index.try_get("header_version_id").map_err(storage_error)?;
    let raw = open_system(tx, r, &version, "checkpoint-header", keys).await?;
    if raw.len() > PAGE
        || hash(&raw)
            != index
                .try_get::<String, _>("original_payload_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let h: CheckpointHeader = serde_json::from_slice(&raw).map_err(storage_error)?;
    if h.format != "android-private-server-checkpoint/1"
        || h.owner_id != id(r, "owner_id")?
        || h.project_id != id(r, "project_id")?
        || h.thread_id != id(r, "thread_id")?
        || h.descriptor.run_id != id(r, "id")?
        || h.descriptor.id != id(index, "id")?
        || h.descriptor.origin != "private_server"
        || index
            .try_get::<String, _>("run_id")
            .map_err(storage_error)?
            != h.descriptor.run_id.to_string()
        || index
            .try_get::<String, _>("owner_id")
            .map_err(storage_error)?
            != h.owner_id.to_string()
        || index
            .try_get::<String, _>("origin")
            .map_err(storage_error)?
            != "private_server"
        || index
            .try_get::<String, _>("source_table")
            .map_err(storage_error)?
            != "android_private_loop_checkpoints"
        || index
            .try_get::<String, _>("source_row_id")
            .map_err(storage_error)?
            != h.descriptor.run_id.to_string()
        || h.spec_sha256 != canonical_hash(&payload(r)?["spec"])
        || h.spec_sha256
            != index
                .try_get::<String, _>("spec_sha256")
                .map_err(storage_error)?
        || h.checkpoint_sha256
            != index
                .try_get::<String, _>("checkpoint_sha256")
                .map_err(storage_error)?
        || h.source_revision
            != index
                .try_get::<i64, _>("source_revision")
                .map_err(storage_error)?
        || h.source_revision != h.descriptor.sequence
        || h.descriptor.sequence != index.try_get::<i64, _>("sequence").map_err(storage_error)?
        || h.original_lease_id
            != index
                .try_get::<Option<String>, _>("original_lease_id")
                .map_err(storage_error)?
        || h.descriptor.state_sha256
            != index
                .try_get::<String, _>("state_sha256")
                .map_err(storage_error)?
        || h.descriptor.state_size
            != index
                .try_get::<i64, _>("state_size")
                .map_err(storage_error)? as u64
        || h.descriptor.safe_to_resume
            != (index
                .try_get::<i64, _>("safe_to_resume")
                .map_err(storage_error)?
                == 1)
        || h.descriptor.created_at
            != device_iso(index.try_get("created_at").map_err(storage_error)?)?
        || index
            .try_get::<String, _>("state_version_id")
            .map_err(storage_error)?
            != version
        || h.descriptor.state_size > 16 * PAGE as u64
        || h.parts.len() > 16
        || h.parts.len() as u32 != h.descriptor.state_pages
        || h.parts.iter().any(|p| p.size == 0 || p.size > PAGE as u64)
        || h.descriptor.state_page_sha256
            != h.parts.iter().map(|p| p.sha256.clone()).collect::<Vec<_>>()
        || h.parts.iter().map(|p| p.size).sum::<u64>() != h.descriptor.state_size
    {
        return Err(Error::Forbidden);
    }
    let binding = json!((
        h.owner_id,
        h.descriptor.run_id,
        h.source_revision,
        &h.checkpoint_sha256
    ));
    if version != system_id("checkpoint-header", &binding)?
        || h.descriptor.id.to_string() != system_id("checkpoint", &binding)?
    {
        return Err(Error::Forbidden);
    }
    Ok(h)
}

/// Complete semantic progress proof, independent of any safe_to_resume flag.
async fn progress(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
) -> Result<String> {
    let run = id(r, "id")?;
    if r.try_get::<Option<String>, _>("effect_intent")
        .map_err(storage_error)?
        .is_some()
    {
        return Err(Error::UncertainEffect);
    }
    let unknown: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state!='received'",
    )
    .bind(run.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_private_loop_requests WHERE run_id=$1 AND state='pending'",
    )
    .bind(run.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if unknown != 0 || pending != 0 {
        return Err(Error::UncertainEffect);
    }
    let steps=sqlx::query("SELECT sequence,kind,invocation_id,request_sha,receipt_sha,lease_id,checkpoint_sha FROM android_private_loop_steps WHERE run_id=$1 ORDER BY sequence LIMIT 20001").bind(run.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if steps.len() > 20000 {
        return Err(Error::BudgetExceeded);
    }
    let mut models = 0i64;
    let mut tools = 0i64;
    let mut last_model = None;
    let mut ledger = Vec::new();
    for s in steps {
        let kind: String = s.try_get("kind").map_err(storage_error)?;
        let sequence: i64 = s.try_get("sequence").map_err(storage_error)?;
        if kind == "model" {
            models += 1;
            last_model = Some((
                sequence,
                s.try_get::<Option<String>, _>("receipt_sha")
                    .map_err(storage_error)?,
            ));
        }
        // Process/MCP catalog observations are not charged as model tools.
        let invocation: String = s.try_get("invocation_id").map_err(storage_error)?;
        if kind == "tool"
            || ((kind == "process" || kind == "mcp") && invocation.starts_with("tool:"))
        {
            tools += 1;
        }
        let receipt: Option<String> = s.try_get("receipt_sha").map_err(storage_error)?;
        if receipt.as_deref().is_none_or(|v| v.len() != 64) {
            return Err(Error::UncertainEffect);
        }
        let original = crate::android_private_tools::step_receipt(tx, r, sequence, keys).await?;
        if receipt.as_deref() != Some(canonical_hash(&original).as_str()) {
            return Err(Error::Forbidden);
        }
        ledger.push(json!((
            sequence,
            kind,
            invocation,
            s.try_get::<String, _>("request_sha")
                .map_err(storage_error)?,
            receipt,
            s.try_get::<String, _>("lease_id").map_err(storage_error)?,
            s.try_get::<String, _>("checkpoint_sha")
                .map_err(storage_error)?
        )));
    }
    if cp["model_steps"].as_i64() != Some(models)
        || cp["tool_steps"].as_i64() != Some(tools)
        || last_model.as_ref().is_some_and(|(seq, sha)| {
            cp["last_model_sequence"].as_i64() != Some(*seq)
                || cp["last_receipt_sha"].as_str() != sha.as_deref()
        })
    {
        return Err(Error::UncertainEffect);
    }
    Ok(canonical_hash(&json!(ledger)))
}

enum Scope {
    Private(AnyRow),
    Device(AnyRow),
}
impl Scope {
    fn row(&self) -> &AnyRow {
        match self {
            Self::Private(r) | Self::Device(r) => r,
        }
    }
    fn origin(&self) -> &str {
        match self {
            Self::Private(_) => "private_server",
            Self::Device(_) => "device_agent",
        }
    }
    fn owner(&self) -> Result<Uuid> {
        id(
            self.row(),
            if matches!(self, Self::Private(_)) {
                "owner_id"
            } else {
                "creator_id"
            },
        )
    }
}
async fn read_scope(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    run: Uuid,
) -> Result<Scope> {
    store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    if let Some(r) = sqlx::query("SELECT * FROM android_private_server_runs WHERE id=$1")
        .bind(run.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        private_actor(store, tx, actor, &r, ContentAction::Read).await?;
        return Ok(Scope::Private(r));
    }
    let r = crate::android_device_runs::device_run_row(tx, run).await?;
    crate::android_device_runs::device_run_permit(store, tx, actor, &r, ContentAction::Read)
        .await?;
    Ok(Scope::Device(r))
}
async fn index_device(tx: &mut Transaction<'_, Any>, r: &AnyRow) -> Result<()> {
    let run = id(r, "id")?;
    let owner = id(r, "creator_id")?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT c.id FROM android_scoped_checkpoints c WHERE c.run_id=$1 AND NOT EXISTS(SELECT 1 FROM android_run_checkpoint_index i WHERE i.run_id=c.run_id AND i.owner_id=$2 AND i.origin='device_agent' AND i.source_table='android_scoped_checkpoints' AND i.source_row_id=c.id) ORDER BY c.id LIMIT 1001",
    )
    .bind(run.to_string())
    .bind(owner.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if ids.len() > 1000 {
        return Err(Error::BudgetExceeded);
    }
    for checkpoint_id in ids {
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM android_scoped_checkpoints WHERE id=$1 AND run_id=$2",
        )
        .bind(&checkpoint_id)
        .bind(run.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if raw.len() > 1024 * 1024 {
            return Err(Error::Forbidden);
        }
        let cp: AndroidRunCheckpoint = parse(&raw)?;
        if cp.run_id != run || cp.sequence <= 0 {
            return Err(Error::Forbidden);
        }
        let state = Zeroizing::new(serde_json::to_vec(&cp.executor_state).map_err(storage_error)?);
        if state.len() > 512 * 1024 {
            return Err(Error::Forbidden);
        }
        let created = chrono::DateTime::parse_from_rfc3339(&cp.created_at)
            .map_err(storage_error)?
            .timestamp_millis();
        let n=sqlx::query("INSERT INTO android_run_checkpoint_index(id,run_id,owner_id,origin,sequence,source_revision,checkpoint_sha256,original_payload_sha256,spec_sha256,state_sha256,state_size,safe_to_resume,created_at,source_table,source_row_id) VALUES($1,$2,$3,'device_agent',$4,$4,$5,$5,$6,$7,$8,$9,$10,'android_scoped_checkpoints',$11) ON CONFLICT DO NOTHING")
            .bind(cp.id.to_string()).bind(run.to_string()).bind(owner.to_string()).bind(cp.sequence).bind(hash(raw.as_bytes())).bind(canonical_hash(&payload(r)?["spec"]))
            .bind(hash(&state)).bind(state.len() as i64).bind(i64::from(cp.safe_to_resume)).bind(created).bind(&checkpoint_id).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
        if n == 0 {
            let old=sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND origin='device_agent'").bind(cp.id.to_string()).bind(run.to_string()).bind(owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            device_checkpoint(tx, r, &old).await?;
        }
    }
    Ok(())
}
pub(crate) async fn capture_device_checkpoint(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
) -> Result<()> {
    index_device(tx, r).await
}
async fn device_checkpoint(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    index: &AnyRow,
) -> Result<AndroidRunCheckpoint> {
    // This public family has current scoped-run authority. An old native-owned
    // device row cannot enter it merely because its UUID happens to match.
    if index
        .try_get::<String, _>("source_table")
        .map_err(storage_error)?
        != "android_scoped_checkpoints"
    {
        return Err(Error::Forbidden);
    }
    let raw: String = sqlx::query_scalar(
        "SELECT payload FROM android_scoped_checkpoints WHERE id=$1 AND run_id=$2",
    )
    .bind(
        index
            .try_get::<String, _>("source_row_id")
            .map_err(storage_error)?,
    )
    .bind(id(r, "id")?.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::Forbidden)?;
    if raw.len() > PAGE
        || hash(raw.as_bytes())
            != index
                .try_get::<String, _>("original_payload_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let cp: AndroidRunCheckpoint = parse(&raw)?;
    let bytes = Zeroizing::new(serde_json::to_vec(&cp.executor_state).map_err(storage_error)?);
    if cp.id != id(index, "id")?
        || cp.run_id != id(r, "id")?
        || cp.sequence != index.try_get::<i64, _>("sequence").map_err(storage_error)?
        || index
            .try_get::<String, _>("owner_id")
            .map_err(storage_error)?
            != id(r, "creator_id")?.to_string()
        || index
            .try_get::<String, _>("origin")
            .map_err(storage_error)?
            != "device_agent"
        || index
            .try_get::<String, _>("spec_sha256")
            .map_err(storage_error)?
            != canonical_hash(&payload(r)?["spec"])
        || hash(&bytes)
            != index
                .try_get::<String, _>("state_sha256")
                .map_err(storage_error)?
        || bytes.len() as i64
            != index
                .try_get::<i64, _>("state_size")
                .map_err(storage_error)?
        || cp.safe_to_resume
            != (index
                .try_get::<i64, _>("safe_to_resume")
                .map_err(storage_error)?
                == 1)
        || chrono::DateTime::parse_from_rfc3339(&cp.created_at)
            .map_err(storage_error)?
            .timestamp_millis()
            != index
                .try_get::<i64, _>("created_at")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(cp)
}
async fn ensure_index(
    tx: &mut Transaction<'_, Any>,
    scope: &Scope,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<()> {
    match scope {
        Scope::Device(r) => index_device(tx, r).await,
        Scope::Private(r) => {
            let latest = sqlx::query(
                "SELECT revision,digest FROM android_private_loop_checkpoints WHERE run_id=$1",
            )
            .bind(id(r, "id")?.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
            if let Some(latest) = latest {
                let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_run_checkpoint_index WHERE run_id=$1 AND owner_id=$2 AND origin='private_server' AND sequence=$3")
                    .bind(id(r,"id")?.to_string()).bind(id(r,"owner_id")?.to_string()).bind(latest.try_get::<i64,_>("revision").map_err(storage_error)?).fetch_one(&mut **tx).await.map_err(storage_error)?;
                if exists == 0 {
                    let cp = crate::android_private_tools::checkpoint(tx, r, keys).await?;
                    capture_checkpoint(tx, r, &cp, keys, key_id, current).await?;
                }
            }
            Ok(())
        }
    }
}
async fn descriptor(
    tx: &mut Transaction<'_, Any>,
    scope: &Scope,
    index: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<AndroidRunCheckpointDescriptor> {
    match scope {
        Scope::Private(r) => Ok(validate_header(tx, r, index, keys).await?.descriptor),
        Scope::Device(r) => {
            let cp = device_checkpoint(tx, r, index).await?;
            let sha: String = index.try_get("state_sha256").map_err(storage_error)?;
            Ok(AndroidRunCheckpointDescriptor {
                id: cp.id,
                run_id: cp.run_id,
                sequence: cp.sequence,
                origin: "device_agent".into(),
                safe_to_resume: cp.safe_to_resume,
                state_sha256: sha.clone(),
                state_size: index
                    .try_get::<i64, _>("state_size")
                    .map_err(storage_error)? as u64,
                state_pages: 1,
                state_page_sha256: vec![sha],
                created_at: cp.created_at,
            })
        }
    }
}
async fn state_part(
    tx: &mut Transaction<'_, Any>,
    scope: &Scope,
    index: &AnyRow,
    page: u32,
    keys: &dyn KeyStore,
) -> Result<Zeroizing<Vec<u8>>> {
    match scope {
        Scope::Device(r) => {
            if page != 0 {
                return Err(Error::NotFound);
            }
            let cp = device_checkpoint(tx, r, index).await?;
            Ok(Zeroizing::new(
                serde_json::to_vec(&cp.executor_state).map_err(storage_error)?,
            ))
        }
        Scope::Private(r) => {
            let h = validate_header(tx, r, index, keys).await?;
            let part = h.parts.get(page as usize).ok_or(Error::NotFound)?;
            let expected = system_id(
                "checkpoint-state",
                &json!((
                    json!((
                        h.owner_id,
                        h.descriptor.run_id,
                        h.source_revision,
                        &h.checkpoint_sha256
                    )),
                    page as usize,
                    &part.sha256
                )),
            )?;
            if part.id != expected {
                return Err(Error::Forbidden);
            }
            let bytes = open_system(tx, r, &part.id, "checkpoint-state", keys).await?;
            if bytes.len() != part.size as usize || hash(&bytes) != part.sha256 {
                return Err(Error::Forbidden);
            }
            Ok(bytes)
        }
    }
}
impl Store {
    pub async fn android_run_checkpoint_page(
        &self,
        actor: &Principal,
        run: Uuid,
        after: i64,
        limit: u32,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<AndroidRunCheckpointPage> {
        if after < 0 || limit == 0 || limit > 20 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let scope = read_scope(self, &mut tx, actor, run).await?;
        ensure_index(
            &mut tx,
            &scope,
            keys,
            workspace::writer_cipher(keys, key_id)?,
            current,
        )
        .await?;
        let rows=sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE run_id=$1 AND owner_id=$2 AND origin=$3 AND sequence>$4 ORDER BY sequence LIMIT $5")
            .bind(run.to_string()).bind(scope.owner()?.to_string()).bind(scope.origin()).bind(after).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let has_more = rows.len() > limit as usize;
        let mut entries = Vec::new();
        for r in rows.iter().take(limit as usize) {
            entries.push(descriptor(&mut tx, &scope, r, keys).await?)
        }
        let next_sequence = if has_more {
            entries.last().map(|v| v.sequence)
        } else {
            None
        };
        read_scope(self, &mut tx, actor, run).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidRunCheckpointPage {
            run_id: run,
            entries,
            next_sequence,
        })
    }
    pub async fn android_run_checkpoint_bytes(
        &self,
        actor: &Principal,
        run: Uuid,
        checkpoint: Uuid,
        page: u32,
        keys: &dyn KeyStore,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let scope = read_scope(self, &mut tx, actor, run).await?;
        let index=sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND origin=$4")
            .bind(checkpoint.to_string()).bind(run.to_string()).bind(scope.owner()?.to_string()).bind(scope.origin()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let bytes = state_part(&mut tx, &scope, &index, page, keys).await?;
        read_scope(self, &mut tx, actor, run).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
    /// Original V1 list shape. Oversized replies fail explicitly; the complete
    /// versioned page/byte endpoints retain every original, without truncation.
    pub async fn android_run_checkpoints(
        &self,
        actor: &Principal,
        run: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Vec<AndroidRunCheckpoint>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let scope = read_scope(self, &mut tx, actor, run).await?;
        ensure_index(
            &mut tx,
            &scope,
            keys,
            workspace::writer_cipher(keys, key_id)?,
            current,
        )
        .await?;
        let rows=sqlx::query("SELECT * FROM android_run_checkpoint_index WHERE run_id=$1 AND owner_id=$2 AND origin=$3 ORDER BY sequence LIMIT 20001")
            .bind(run.to_string()).bind(scope.owner()?.to_string()).bind(scope.origin()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 20000
            || rows.iter().try_fold(0u64, |n, r| {
                n.checked_add(
                    r.try_get::<i64, _>("state_size").map_err(storage_error)? as u64 + 1024,
                )
                .ok_or(Error::BudgetExceeded)
            })? > 3 * PAGE as u64
        {
            return Err(Error::Validation("checkpoint_page_required: all original checkpoints remain available through /checkpoints/page and checkpoint state byte pages".into()));
        }
        let mut values = Vec::new();
        for r in rows {
            if let Scope::Device(device) = &scope {
                values.push(device_checkpoint(&mut tx, device, &r).await?);
                continue;
            }
            let d = descriptor(&mut tx, &scope, &r, keys).await?;
            let mut state = Zeroizing::new(Vec::new());
            for page in 0..d.state_pages {
                state.extend_from_slice(&state_part(&mut tx, &scope, &r, page, keys).await?);
            }
            if state.len() != d.state_size as usize || hash(&state) != d.state_sha256 {
                return Err(Error::Forbidden);
            }
            values.push(AndroidRunCheckpoint {
                schema_version: 2,
                id: d.id,
                run_id: run,
                sequence: d.sequence,
                safe_to_resume: d.safe_to_resume,
                executor_state: serde_json::from_slice(&state).map_err(storage_error)?,
                created_at: d.created_at,
            });
        }
        if serde_json::to_vec(&values).map_err(storage_error)?.len() > 3 * PAGE {
            return Err(Error::BudgetExceeded);
        }
        read_scope(self, &mut tx, actor, run).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
}

pub(crate) async fn clear_waiting(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    owner: Uuid,
    family: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM android_run_waiting_guards WHERE run_id=$1 AND owner_id=$2 AND family=$3",
    )
    .bind(run.to_string())
    .bind(owner.to_string())
    .bind(family)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn record_private_waiting(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    reason: AndroidRunWaitingReason,
    host: Option<&str>,
    current: i64,
) -> Result<()> {
    let state: String = r.try_get("state").map_err(storage_error)?;
    if !matches!(state.as_str(), "queued" | "waiting_for_snapshot") {
        return Err(Error::Conflict);
    }
    let reason = serde_json::to_value(reason).map_err(storage_error)?;
    sqlx::query("INSERT INTO android_run_waiting_guards(run_id,owner_id,family,run_revision,state,lease_id,host_slot,reason,checked_at) VALUES($1,$2,'private_server',$3,$4,$5,$6,$7,$8) ON CONFLICT(run_id,owner_id,family) DO UPDATE SET run_revision=excluded.run_revision,state=excluded.state,lease_id=excluded.lease_id,host_slot=excluded.host_slot,reason=excluded.reason,checked_at=excluded.checked_at")
        .bind(id(r,"id")?.to_string()).bind(id(r,"owner_id")?.to_string()).bind(revision(r)?).bind(state).bind(r.try_get::<Option<String>,_>("lease_id").map_err(storage_error)?)
        .bind(host).bind(reason.as_str().ok_or_else(device_invalid)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn snapshot_waiting(
    tx: &mut Transaction<'_, Any>,
    scope: &Scope,
    current: i64,
) -> Result<()> {
    let r = scope.row();
    if r.try_get::<String, _>("state").map_err(storage_error)? != "waiting_for_snapshot" {
        return Ok(());
    }
    let spec = payload(r)?;
    let snapshot = spec["spec"]["snapshot_id"]
        .as_str()
        .ok_or(Error::Conflict)?;
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0 AND committed=0")
        .bind(snapshot).bind(scope.owner()?.to_string()).bind(id(r,"project_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 1 {
        return Ok(());
    }
    let family = scope.origin();
    let run = id(r, "id")?;
    // This is a fresh observation of the actual admitted snapshot's uncommitted
    // row, not a reason guessed from a queued status or an old event.
    sqlx::query("INSERT INTO android_run_waiting_guards(run_id,owner_id,family,run_revision,state,lease_id,host_slot,reason,checked_at) VALUES($1,$2,$3,$4,'waiting_for_snapshot',NULL,NULL,'admission_unavailable',$5) ON CONFLICT(run_id,owner_id,family) DO UPDATE SET run_revision=excluded.run_revision,state=excluded.state,lease_id=NULL,host_slot=NULL,reason=excluded.reason,checked_at=excluded.checked_at")
        .bind(run.to_string()).bind(scope.owner()?.to_string()).bind(family).bind(revision(r)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    /// Only the real worker calls this after all four local slots are occupied.
    /// The per-service observer cannot be adopted by a different/restarted node.
    pub async fn android_private_observe_capacity(
        &self,
        observer: &str,
        runner: Option<&Value>,
    ) -> Result<()> {
        if observer.len() > 200 || observer.is_empty() {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let rows=sqlx::query("SELECT * FROM android_private_server_runs WHERE state='queued' ORDER BY created_at LIMIT 100").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            let wire = payload(&r)?;
            let expected = &wire["spec"]["input"]["frozen_private_runner"];
            if !expected.is_null() && runner != Some(expected) {
                continue;
            }
            if crate::android_private_server::authority(&mut tx, &r, current)
                .await
                .is_ok()
            {
                record_private_waiting(
                    &mut tx,
                    &r,
                    AndroidRunWaitingReason::InstanceCapacity,
                    Some(observer),
                    current,
                )
                .await?;
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_clear_observer_waiting(&self, observer: &str) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        sqlx::query("DELETE FROM android_run_waiting_guards WHERE family='private_server' AND host_slot=$1 AND reason IN('instance_capacity','admission_unavailable')")
            .bind(observer).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// The actual worker calls this only after original runner recovery failed.
    pub async fn android_private_observe_unavailable(&self, observer: &str) -> Result<()> {
        if observer.is_empty() || observer.len() > 200 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let rows=sqlx::query("SELECT * FROM android_private_server_runs WHERE state='queued' ORDER BY created_at LIMIT 100").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            record_private_waiting(
                &mut tx,
                &r,
                AndroidRunWaitingReason::AdmissionUnavailable,
                Some(observer),
                current,
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_run_waiting(
        &self,
        actor: &Principal,
        run: Uuid,
        observer: &str,
    ) -> Result<AndroidRunWaiting> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let scope = read_scope(self, &mut tx, actor, run).await?;
        snapshot_waiting(&mut tx, &scope, current).await?;
        let guard=sqlx::query("SELECT * FROM android_run_waiting_guards WHERE run_id=$1 AND owner_id=$2 AND family=$3")
            .bind(run.to_string()).bind(scope.owner()?.to_string()).bind(scope.origin()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let mut result = AndroidRunWaiting {
            run_id: run,
            reason: None,
            checked_at: None,
        };
        if let Some(g) = guard {
            let r = scope.row();
            let state: String = r.try_get("state").map_err(storage_error)?;
            let lease: Option<String> = match &scope {
                Scope::Private(_) => r.try_get("lease_id"),
                Scope::Device(_) => r.try_get("lease_token"),
            }
            .map_err(storage_error)?;
            let host: Option<String> = g.try_get("host_slot").map_err(storage_error)?;
            if g.try_get::<i64, _>("run_revision").map_err(storage_error)? == revision(r)?
                && g.try_get::<String, _>("state").map_err(storage_error)? == state
                && matches!(state.as_str(), "queued" | "waiting_for_snapshot")
                && g.try_get::<Option<String>, _>("lease_id")
                    .map_err(storage_error)?
                    == lease
                && host.as_deref().is_none_or(|h| h == observer)
            {
                result.reason = Some(
                    serde_json::from_value(json!(
                        g.try_get::<String, _>("reason").map_err(storage_error)?
                    ))
                    .map_err(storage_error)?,
                );
                result.checked_at =
                    Some(device_iso(g.try_get("checked_at").map_err(storage_error)?)?);
                if matches!(
                    result.reason,
                    Some(AndroidRunWaitingReason::ChatWorkspaceBusy)
                ) {
                    let blocked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_server_runs WHERE id<>$1 AND owner_id=$2 AND thread_id=$3 AND state IN('running','waiting_for_approval','waiting_for_input') AND lease_until>$4")
                        .bind(run.to_string()).bind(scope.owner()?.to_string()).bind(id(r,"thread_id")?.to_string()).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
                    if blocked == 0 {
                        result.reason = None;
                        result.checked_at = None;
                    }
                }
            }
        }
        read_scope(self, &mut tx, actor, run).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeOriginal {
    format: String,
    owner_id: Uuid,
    project_id: Uuid,
    run_id: Uuid,
    request: AndroidResumeRun,
    original_session_id: Uuid,
    original_session_revision: i64,
    admitted_session_id: Uuid,
    admitted_session_revision: i64,
    source_run_sha256: String,
    spec_sha256: String,
    checkpoint_sha256: String,
    checkpoint_source_revision: i64,
    progress_sha256: String,
    physical_intents_sha256: String,
    physical_receipts_sha256: String,
    receipt: AndroidRunResumeReceipt,
}
/// Internal only: no Deserialize, caller-supplied checkpoint or safe Boolean.
pub struct AndroidPrivateResumeSnapshot {
    run: Uuid,
    owner: Uuid,
    source_revision: i64,
    run_sha: String,
    spec_sha: String,
    checkpoint_id: Uuid,
    checkpoint_sha: String,
    checkpoint_revision: i64,
    progress_sha: String,
    physical_intents_sha: String,
    requires_cleanup: bool,
}
impl AndroidPrivateResumeSnapshot {
    pub fn requires_cleanup(&self) -> bool {
        self.requires_cleanup
    }
}
pub enum AndroidPrivateResumePrepared {
    Original(AndroidRunResumeReceipt),
    Pending(AndroidPrivateResumeSnapshot),
}

/// A continuation always validates its original immutable envelope locator and
/// current identity/project tuple. It never edits the original admission row.
pub(crate) async fn continuation_session(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    current: i64,
) -> Result<Option<(Uuid, i64)>> {
    let c=sqlx::query("SELECT c.*,v.run_id AS version_run,v.path AS version_path,v.kind AS version_kind,v.digest AS version_sha,i.owner_id AS checkpoint_owner,i.run_id AS checkpoint_run,i.origin AS checkpoint_origin,i.checkpoint_sha256 AS checkpoint_digest FROM android_private_run_continuations c JOIN android_private_loop_versions v ON v.id=c.resume_version_id JOIN android_run_checkpoint_index i ON i.id=c.checkpoint_id WHERE c.run_id=$1")
        .bind(id(r,"id")?.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(c) = c else { return Ok(None) };
    let owner = id(r, "owner_id")?;
    let run = id(r, "id")?;
    let request = id(&c, "request_id")?;
    let version: String = c.try_get("resume_version_id").map_err(storage_error)?;
    if id(&c, "owner_id")? != owner
        || id(&c, "original_session_id")? != id(r, "session_id")?
        || c.try_get::<i64, _>("original_session_revision")
            .map_err(storage_error)?
            != r.try_get::<i64, _>("session_revision")
                .map_err(storage_error)?
        || c.try_get::<String, _>("spec_sha256")
            .map_err(storage_error)?
            != canonical_hash(&payload(r)?["spec"])
        || c.try_get::<String, _>("version_sha")
            .map_err(storage_error)?
            != c.try_get::<String, _>("resume_sha256")
                .map_err(storage_error)?
        || c.try_get::<String, _>("version_run")
            .map_err(storage_error)?
            != run.to_string()
        || c.try_get::<String, _>("version_kind")
            .map_err(storage_error)?
            != "artifact"
        || version != system_id("resume", &json!((owner, request)))?
        || c.try_get::<String, _>("version_path")
            .map_err(storage_error)?
            != format!("{SYSTEM}/resume/{version}")
        || c.try_get::<String, _>("checkpoint_owner")
            .map_err(storage_error)?
            != owner.to_string()
        || c.try_get::<String, _>("checkpoint_run")
            .map_err(storage_error)?
            != run.to_string()
        || c.try_get::<String, _>("checkpoint_origin")
            .map_err(storage_error)?
            != "private_server"
        || c.try_get::<String, _>("checkpoint_digest")
            .map_err(storage_error)?
            != c.try_get::<String, _>("checkpoint_sha256")
                .map_err(storage_error)?
        || revision(r)?
            < c.try_get::<i64, _>("queued_revision")
                .map_err(storage_error)?
        || c.try_get::<i64, _>("project_revision")
            .map_err(storage_error)?
            != r.try_get::<i64, _>("project_revision")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let session = id(&c, "session_id")?;
    let session_revision: i64 = c.try_get("session_revision").map_err(storage_error)?;
    let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN android_personal_projects p ON p.owner_id=i.id JOIN android_personal_threads t ON t.owner_id=i.id AND t.project_id=p.id WHERE i.id=$1 AND i.active=1 AND s.id=$2 AND s.revision=$3 AND s.revoked=0 AND s.expires_at>$4 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id) AND p.id=$5 AND p.revision=$6 AND p.deleted=0 AND t.id=$7 AND t.deleted=0")
        .bind(owner.to_string()).bind(session.to_string()).bind(session_revision).bind(current).bind(id(r,"project_id")?.to_string()).bind(c.try_get::<i64,_>("project_revision").map_err(storage_error)?).bind(id(r,"thread_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if valid != 1 {
        return Err(Error::Forbidden);
    }
    Ok(Some((session, session_revision)))
}
async fn resume_configuration(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    r: &AnyRow,
    cp: &Value,
    current: i64,
) -> Result<i64> {
    private_actor(store, tx, actor, r, ContentAction::Execute).await?;
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    // Preserve any real scheduled origin. A fresh personal session is no
    // substitute for a revoked original schedule grant/configuration.
    crate::android_private_schedules::validate_origin(tx, r, "server_linux", current).await?;
    let session_revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3")
        .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let frozen = payload(r)?;
    if frozen["spec"]["executor_target"]["kind"] != "server_linux"
        || frozen["spec"]["creator_user_id"] != actor.identity_id().to_string()
    {
        return Err(Error::Forbidden);
    }
    let actual_project: i64 = sqlx::query_scalar(
        "SELECT revision FROM android_personal_projects WHERE id=$1 AND owner_id=$2 AND deleted=0",
    )
    .bind(id(r, "project_id")?.to_string())
    .bind(actor.identity_id().to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if actual_project
        != r.try_get::<i64, _>("project_revision")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let profile = crate::android_private_server::authority_configuration(tx, r, current).await?;
    let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
    crate::android_server_capabilities::validate_private_spec(&frozen, &spec)?;
    if spec.executor != "android_private_server_linux"
        || spec.order_id != id(r, "thread_id")?.to_string()
        || frozen["spec"]["input"]["model_binding"]
            != serde_json::to_value(&spec.model_binding).map_err(storage_error)?
        || cp["deadline"].as_i64().is_none_or(|d| d <= current)
    {
        return Err(Error::Forbidden);
    }
    let thread_revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0",
    )
    .bind(id(r, "thread_id")?.to_string())
    .bind(actor.identity_id().to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if cp["thread_revision"].as_i64() != Some(thread_revision) {
        return Err(Error::Forbidden);
    }
    if let Some(model) = crate::actor_model_registry::check_binding(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        &profile,
        &spec,
    )
    .await?
    {
        crate::actor_model_registry::validate_context(
            &model,
            &spec,
            cp["messages"].as_array().ok_or_else(device_invalid)?,
            &model_request_fingerprint(
                &spec,
                cp["messages"].as_array().ok_or_else(device_invalid)?,
                &model_tool_catalog(&spec),
            )?,
        )
        .map_err(|_| Error::Forbidden)?;
    }
    Ok(session_revision)
}
async fn physical(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    complete: bool,
) -> Result<(String, String)> {
    let run = id(r, "id")?;
    let containers = sqlx::query(
        "SELECT * FROM android_private_containers WHERE run_id=$1 ORDER BY id LIMIT 10001",
    )
    .bind(run.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if containers.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    let mut intents = Vec::new();
    let mut receipts = Vec::new();
    for c in containers {
        let nonce = id(&c, "id")?;
        let binding: Value = parse(
            &c.try_get::<String, _>("original_binding")
                .map_err(storage_error)?,
        )?;
        if id(&c, "owner_id")? != id(r, "owner_id")?
            || id(&c, "project_id")? != id(r, "project_id")?
            || binding != payload(r)?["spec"]["input"]["frozen_private_runner"]
        {
            return Err(Error::Forbidden);
        }
        intents.push(json!((
            nonce,
            id(&c, "run_id")?,
            id(&c, "owner_id")?,
            id(&c, "project_id")?,
            id(&c, "lease_id")?,
            c.try_get::<String, _>("invocation_id")
                .map_err(storage_error)?,
            canonical_hash(&binding),
            c.try_get::<String, _>("creation_sha")
                .map_err(storage_error)?
        )));
        if !complete {
            continue;
        }
        let never = if c
            .try_get::<Option<String>, _>("original_unsent_receipt")
            .map_err(storage_error)?
            .is_some()
        {
            store
                .android_private_container_no_create_receipt(tx, nonce)
                .await?
        } else {
            None
        };
        if let Some(never) = never {
            receipts.push(json!({"nonce":nonce,"never_create_receipt":canonical_hash(&never),"physical_evidence":false}));
            continue;
        }
        let witness: Value = parse(
            &c.try_get::<Option<String>, _>("termination_witness")
                .map_err(storage_error)?
                .ok_or(Error::UncertainEffect)?,
        )?;
        let container: Option<String> = c.try_get("container_id").map_err(storage_error)?;
        if c.try_get::<String, _>("state").map_err(storage_error)? != "removed"
            || witness["removed"] != true
            || witness["nonce"] != nonce.to_string()
            || witness["engine_id"] != binding["engine_id"]
            || witness["original_id"] != json!(container)
            || witness["running"] != false
            || witness["paused"] != false
            || witness["restarting"] != false
            || witness["pid"] != 0
            || witness["stop"]["nonce"] != nonce.to_string()
            || witness["stop"]["engine_id"] != binding["engine_id"]
            || witness["stop"]["original_id"] != witness["original_id"]
            || witness["stop"]["running"] != false
            || witness["stop"]["paused"] != false
            || witness["stop"]["restarting"] != false
            || witness["stop"]["pid"] != 0
        {
            return Err(Error::UncertainEffect);
        }
        receipts.push(json!({"nonce":nonce,"original_physical_receipt":canonical_hash(&witness),"physical_evidence":true}));
    }
    // A closed container alone cannot settle an original transport/request.
    for table in [
        "android_private_terminal_sessions",
        "android_private_browser_sessions",
        "android_private_github_sessions",
    ] {
        let query = format!(
            "SELECT id,owner_id,project_id,lease_id,container_nonce FROM {table} WHERE run_id=$1 ORDER BY id LIMIT 10001"
        );
        let sessions = sqlx::query(&query)
            .bind(run.to_string())
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if sessions.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        for session in sessions {
            if id(&session, "owner_id")? != id(r, "owner_id")?
                || id(&session, "project_id")? != id(r, "project_id")?
            {
                return Err(Error::Forbidden);
            }
            intents.push(json!((
                table,
                id(&session, "id")?,
                id(&session, "owner_id")?,
                id(&session, "project_id")?,
                id(&session, "lease_id")?,
                session
                    .try_get::<Option<String>, _>("container_nonce")
                    .map_err(storage_error)?
            )));
        }
        if complete {
            let query = format!("SELECT COUNT(*) FROM {table} WHERE run_id=$1 AND state!='closed'");
            let n: i64 = sqlx::query_scalar(&query)
                .bind(run.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
            if n != 0 {
                return Err(Error::UncertainEffect);
            }
        }
    }
    for query in [
        "SELECT COUNT(*) FROM android_private_mcp_sessions WHERE run_id=$1 AND (state!='ready' OR protocol='2024-11-05' OR checkpoint LIKE '%\"transport\":\"stdio\"%')",
        "SELECT COUNT(*) FROM android_private_browser_requests q JOIN android_private_browser_sessions s ON s.id=q.session_id WHERE s.run_id=$1 AND q.state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_github_requests q JOIN android_private_github_sessions s ON s.id=q.session_id WHERE s.run_id=$1 AND q.state IN('admitted','uncertain')",
    ] {
        let n: i64 = sqlx::query_scalar(query)
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if n != 0 {
            return Err(Error::UncertainEffect);
        }
    }
    Ok((
        canonical_hash(&json!(intents)),
        canonical_hash(&json!(receipts)),
    ))
}
async fn original_resume(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    actor: &Principal,
    request: &AndroidResumeRun,
    keys: &dyn KeyStore,
) -> Result<Option<AndroidRunResumeReceipt>> {
    let version = system_id("resume", &json!((actor.identity_id(), request.request_id)))?;
    let original = sqlx::query("SELECT run_id FROM android_private_loop_versions WHERE id=$1")
        .bind(&version)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(original) = original else {
        return Ok(None);
    };
    if original
        .try_get::<String, _>("run_id")
        .map_err(storage_error)?
        != id(r, "id")?.to_string()
    {
        return Err(Error::Conflict);
    }
    let raw = open_system(tx, r, &version, "resume", keys).await?;
    let v: ResumeOriginal = serde_json::from_slice(&raw).map_err(storage_error)?;
    if v.format != "android-private-server-resume/1"
        || v.owner_id != actor.identity_id()
        || v.run_id != id(r, "id")?
        || v.project_id != id(r, "project_id")?
        || v.request.request_id != request.request_id
        || v.request.expected_revision != request.expected_revision
        || v.original_session_id != id(r, "session_id")?
        || v.original_session_revision
            != r.try_get::<i64, _>("session_revision")
                .map_err(storage_error)?
        || v.spec_sha256 != canonical_hash(&payload(r)?["spec"])
        || v.receipt.run_id != v.run_id
        || v.receipt.request_id != request.request_id
        || v.receipt.queued_revision
            != request
                .expected_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    Ok(Some(v.receipt))
}
impl Store {
    pub async fn android_private_run_resume_prepare(
        &self,
        actor: &Principal,
        run: Uuid,
        request: &AndroidResumeRun,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<AndroidPrivateResumePrepared> {
        if request.request_id.is_nil() || request.expected_revision <= 0 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        private_actor(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        if let Some(original) = original_resume(&mut tx, &r, actor, request, keys).await? {
            private_actor(self, &mut tx, actor, &r, ContentAction::Execute).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(AndroidPrivateResumePrepared::Original(original));
        }
        if revision(&r)? != request.expected_revision
            || r.try_get::<String, _>("state").map_err(storage_error)? != "interrupted"
            || r.try_get::<i64, _>("lease_until").map_err(storage_error)? > current
        {
            return Err(Error::Conflict);
        }
        let cp = crate::android_private_tools::checkpoint(&mut tx, &r, keys).await?;
        resume_configuration(self, &mut tx, actor, &r, &cp, current).await?;
        capture_checkpoint(
            &mut tx,
            &r,
            &cp,
            keys,
            workspace::writer_cipher(keys, key_id)?,
            current,
        )
        .await?;
        let source = sqlx::query(
            "SELECT digest,revision FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let checkpoint_revision: i64 = source.try_get("revision").map_err(storage_error)?;
        let checkpoint_sha: String = source.try_get("digest").map_err(storage_error)?;
        let checkpoint_id = Uuid::parse_str(&system_id(
            "checkpoint",
            &json!((
                actor.identity_id(),
                run,
                checkpoint_revision,
                &checkpoint_sha
            )),
        )?)
        .map_err(storage_error)?;
        let progress_sha = progress(&mut tx, &r, &cp, keys).await?;
        let (physical_intents_sha, _) = physical(self, &mut tx, &r, false).await?;
        let remaining:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE run_id=$1 AND state!='removed' AND original_unsent_receipt IS NULL").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let sessions:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_private_terminal_sessions WHERE run_id=$1 AND state!='closed')+(SELECT COUNT(*) FROM android_private_browser_sessions WHERE run_id=$1 AND state!='closed')+(SELECT COUNT(*) FROM android_private_github_sessions WHERE run_id=$1 AND state!='closed')")
            .bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        resume_configuration(self, &mut tx, actor, &r, &cp, current).await?;
        let snapshot = AndroidPrivateResumeSnapshot {
            run,
            owner: actor.identity_id(),
            source_revision: request.expected_revision,
            run_sha: hash(
                r.try_get::<String, _>("payload")
                    .map_err(storage_error)?
                    .as_bytes(),
            ),
            spec_sha: canonical_hash(&payload(&r)?["spec"]),
            checkpoint_id,
            checkpoint_sha,
            checkpoint_revision,
            progress_sha,
            physical_intents_sha,
            requires_cleanup: remaining != 0 || sessions != 0,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidPrivateResumePrepared::Pending(snapshot))
    }
    pub async fn android_private_run_resume_finish(
        &self,
        actor: &Principal,
        request: &AndroidResumeRun,
        snapshot: &AndroidPrivateResumeSnapshot,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<AndroidRunResumeReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        crate::server_keys::cipher_writer(&mut tx, workspace::writer_cipher(keys, key_id)?).await?;
        let r = row(&mut tx, snapshot.run).await?;
        private_actor(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        if let Some(original) = original_resume(&mut tx, &r, actor, request, keys).await? {
            private_actor(self, &mut tx, actor, &r, ContentAction::Execute).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        if snapshot.owner != actor.identity_id()
            || request.expected_revision != snapshot.source_revision
            || revision(&r)? != snapshot.source_revision
            || r.try_get::<String, _>("state").map_err(storage_error)? != "interrupted"
            || r.try_get::<i64, _>("lease_until").map_err(storage_error)? > current
            || hash(
                r.try_get::<String, _>("payload")
                    .map_err(storage_error)?
                    .as_bytes(),
            ) != snapshot.run_sha
            || canonical_hash(&payload(&r)?["spec"]) != snapshot.spec_sha
        {
            return Err(Error::Conflict);
        }
        let cp = crate::android_private_tools::checkpoint(&mut tx, &r, keys).await?;
        let cp_row = sqlx::query(
            "SELECT digest,revision FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(snapshot.run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if cp_row
            .try_get::<String, _>("digest")
            .map_err(storage_error)?
            != snapshot.checkpoint_sha
            || cp_row
                .try_get::<i64, _>("revision")
                .map_err(storage_error)?
                != snapshot.checkpoint_revision
            || progress(&mut tx, &r, &cp, keys).await? != snapshot.progress_sha
        {
            return Err(Error::Conflict);
        }
        let (physical_intents, physical_receipts) = physical(self, &mut tx, &r, true).await?;
        if physical_intents != snapshot.physical_intents_sha {
            return Err(Error::Conflict);
        }
        let current = device_writer(&mut tx).await?;
        let session_revision = resume_configuration(self, &mut tx, actor, &r, &cp, current).await?;
        let queued_revision = request
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let receipt = AndroidRunResumeReceipt {
            request_id: request.request_id,
            run_id: snapshot.run,
            checkpoint_id: snapshot.checkpoint_id,
            queued_revision,
        };
        let original = ResumeOriginal {
            format: "android-private-server-resume/1".into(),
            owner_id: actor.identity_id(),
            project_id: id(&r, "project_id")?,
            run_id: snapshot.run,
            request: request.clone(),
            original_session_id: id(&r, "session_id")?,
            original_session_revision: r.try_get("session_revision").map_err(storage_error)?,
            admitted_session_id: actor.session_id(),
            admitted_session_revision: session_revision,
            source_run_sha256: snapshot.run_sha.clone(),
            spec_sha256: snapshot.spec_sha.clone(),
            checkpoint_sha256: snapshot.checkpoint_sha.clone(),
            checkpoint_source_revision: snapshot.checkpoint_revision,
            progress_sha256: snapshot.progress_sha.clone(),
            physical_intents_sha256: physical_intents,
            physical_receipts_sha256: physical_receipts,
            receipt: receipt.clone(),
        };
        let raw = Zeroizing::new(serde_json::to_vec(&original).map_err(storage_error)?);
        let version = system_id("resume", &json!((actor.identity_id(), request.request_id)))?;
        system_version(
            &mut tx,
            &r,
            &version,
            "resume",
            &raw,
            keys,
            workspace::writer_cipher(keys, key_id)?,
            current,
        )
        .await?;
        let n=sqlx::query("INSERT INTO android_private_run_continuations(run_id,owner_id,request_id,request_sha256,resume_version_id,resume_sha256,source_revision,queued_revision,checkpoint_id,checkpoint_sha256,spec_sha256,original_session_id,original_session_revision,session_id,session_revision,project_revision,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) ON CONFLICT(run_id) DO UPDATE SET request_id=excluded.request_id,request_sha256=excluded.request_sha256,resume_version_id=excluded.resume_version_id,resume_sha256=excluded.resume_sha256,source_revision=excluded.source_revision,queued_revision=excluded.queued_revision,checkpoint_id=excluded.checkpoint_id,checkpoint_sha256=excluded.checkpoint_sha256,spec_sha256=excluded.spec_sha256,session_id=excluded.session_id,session_revision=excluded.session_revision,project_revision=excluded.project_revision,created_at=excluded.created_at WHERE android_private_run_continuations.owner_id=excluded.owner_id AND android_private_run_continuations.queued_revision<=excluded.source_revision AND android_private_run_continuations.original_session_id=excluded.original_session_id AND android_private_run_continuations.original_session_revision=excluded.original_session_revision")
            .bind(snapshot.run.to_string()).bind(actor.identity_id().to_string()).bind(request.request_id.to_string()).bind(canonical_hash(&json!(request))).bind(&version).bind(hash(&raw)).bind(snapshot.source_revision).bind(queued_revision).bind(snapshot.checkpoint_id.to_string()).bind(&snapshot.checkpoint_sha).bind(&snapshot.spec_sha).bind(original.original_session_id.to_string()).bind(original.original_session_revision).bind(actor.session_id().to_string()).bind(session_revision).bind(r.try_get::<i64,_>("project_revision").map_err(storage_error)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if n != 1 {
            return Err(Error::Conflict);
        }
        let mut wire = payload(&r)?;
        wire["finished_at"] = Value::Null;
        wire["error"] = Value::Null;
        wire["result"] = Value::Null;
        wire["assigned_executor_id"] = Value::Null;
        wire["lease_expires_at"] = Value::Null;
        let changed=sqlx::query("UPDATE android_private_server_runs SET payload=$1,lease_id=NULL,lease_until=0 WHERE id=$2 AND state='interrupted' AND payload=$3")
            .bind(json_string(&wire)?).bind(snapshot.run.to_string()).bind(r.try_get::<String,_>("payload").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let staged = row(&mut tx, snapshot.run).await?;
        let queued = save(&mut tx, &staged, "queued", None, None, current).await?;
        if queued["revision"].as_i64() != Some(queued_revision) {
            return Err(Error::Conflict);
        }
        let latest = row(&mut tx, snapshot.run).await?;
        let final_current = device_writer(&mut tx).await?;
        continuation_session(&mut tx, &latest, final_current).await?;
        crate::android_private_server::authority(&mut tx, &latest, final_current).await?;
        workspace::bounds(&mut tx, snapshot.run).await?;
        crate::android_private_server::event(&mut tx,snapshot.run,"private_server_resume_admitted",&json!({"request_id":request.request_id,"checkpoint_id":snapshot.checkpoint_id,"source_revision":snapshot.source_revision,"queued_revision":queued_revision,"original_receipt_sha256":hash(&raw)}),current).await?;
        clear_waiting(&mut tx, snapshot.run, actor.identity_id(), "private_server").await?;
        private_actor(self, &mut tx, actor, &latest, ContentAction::Execute).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
