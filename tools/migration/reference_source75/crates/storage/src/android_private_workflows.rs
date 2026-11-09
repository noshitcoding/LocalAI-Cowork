//! Private Actor registry. Native project/run registries are never consulted.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_contracts::{
    AndroidWorkflowChange, AndroidWorkflowEngine, AndroidWorkflowEntry,
    AndroidWorkflowOperationReceipt, AndroidWorkflowPage, AndroidWorkflowSelection,
    AndroidWorkflowVersion, AndroidWorkflowVersionRef, AndroidWorkflowWrite,
};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::Zeroizing;
#[path = "android_private_workflow_cipher.rs"]
mod cipher;
#[path = "android_private_workflow_content.rs"]
mod content;
#[path = "android_private_workflow_definition.rs"]
pub(crate) mod definition;
#[path = "android_private_workflow_engines.rs"]
pub(crate) mod engines;
#[path = "android_private_workflow_execution.rs"]
pub(crate) mod execution;
#[path = "android_private_workflow_proof.rs"]
pub(crate) mod proof;
#[path = "android_private_workflow_rpc.rs"]
mod rpc;
pub use rpc::{PrivateCrewRpc, PrivateCrewRpcPermit};
#[path = "android_private_workflow_control.rs"]
mod control;
pub(crate) use cipher::{
    preflight as workflow_preflight, required_keys as workflow_required_keys,
    rewrap as workflow_rewrap,
};
pub(crate) use content::{RegistryContent, content_page, import_draft_with_id_in_tx};
pub use execution::current_private_workflow_node;
pub use execution::{PrivateWorkflowNodeScope, with_private_workflow_node};
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_REGISTRY_BYTES: i64 = 256 * 1024 * 1024;
fn uuid(r: &AnyRow, k: &str) -> Result<Uuid> {
    Uuid::parse_str(&r.try_get::<String, _>(k).map_err(storage_error)?).map_err(storage_error)
}
fn engine(v: &AndroidWorkflowEngine) -> &'static str {
    match v {
        AndroidWorkflowEngine::Workflow => "workflow",
        AndroidWorkflowEngine::Crew => "crew",
    }
}
async fn head(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    id: Uuid,
) -> Result<AnyRow> {
    sqlx::query(
        "SELECT * FROM android_private_workflows WHERE id=$1 AND owner_id=$2 AND project_id=$3",
    )
    .bind(id.to_string())
    .bind(owner.to_string())
    .bind(project.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)
}
async fn version_ref(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    vid: Uuid,
) -> Result<AndroidWorkflowVersionRef> {
    let v=sqlx::query("SELECT * FROM android_private_workflow_versions WHERE id=$1 AND workflow_id=$2 AND owner_id=$3 AND project_id=$4").bind(vid.to_string()).bind(uuid(r,"id")?.to_string()).bind(uuid(r,"owner_id")?.to_string()).bind(uuid(r,"project_id")?.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    Ok(AndroidWorkflowVersionRef {
        id: vid,
        revision: v.try_get("revision").map_err(storage_error)?,
        definition_sha256: v.try_get("definition_sha256").map_err(storage_error)?,
        created_at: device_iso(v.try_get("created_at").map_err(storage_error)?)?,
    })
}
async fn entry(tx: &mut Transaction<'_, Any>, r: &AnyRow) -> Result<AndroidWorkflowEntry> {
    let published = r
        .try_get::<Option<String>, _>("published_version_id")
        .map_err(storage_error)?
        .map(|v| Uuid::parse_str(&v).map_err(storage_error))
        .transpose()?;
    Ok(AndroidWorkflowEntry {
        id: uuid(r, "id")?,
        owner_id: uuid(r, "owner_id")?,
        project_id: uuid(r, "project_id")?,
        name: r.try_get("name").map_err(storage_error)?,
        engine: match r
            .try_get::<String, _>("engine")
            .map_err(storage_error)?
            .as_str()
        {
            "workflow" => AndroidWorkflowEngine::Workflow,
            "crew" => AndroidWorkflowEngine::Crew,
            _ => return Err(Error::Forbidden),
        },
        revision: r.try_get("revision").map_err(storage_error)?,
        revoked: r.try_get::<i64, _>("revoked").map_err(storage_error)? != 0,
        published_version: match published {
            Some(v) => Some(version_ref(tx, r, v).await?),
            None => None,
        },
        source_entity_id: r
            .try_get::<Option<String>, _>("source_entity_id")
            .map_err(storage_error)?
            .map(|v| Uuid::parse_str(&v).map_err(storage_error))
            .transpose()?,
        source_revision: r.try_get("source_revision").map_err(storage_error)?,
    })
}
async fn quota(tx: &mut Transaction<'_, Any>, owner: Uuid, add: usize) -> Result<()> {
    let bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size),0) FROM android_private_workflow_chunks WHERE owner_id=$1",
    )
    .bind(owner.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let versions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_private_workflow_versions WHERE owner_id=$1",
    )
    .bind(owner.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if bytes
        .checked_add(i64::try_from(add).map_err(|_| device_invalid())?)
        .is_none_or(|n| n > MAX_REGISTRY_BYTES)
        || versions >= 10000
    {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
async fn original(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
    request: Uuid,
    purpose: &str,
    value: &Value,
    keys: &dyn KeyStore,
) -> Result<Option<AndroidWorkflowOperationReceipt>> {
    let Some(r) = sqlx::query(
        "SELECT * FROM android_private_workflow_operations WHERE owner_id=$1 AND request_id=$2",
    )
    .bind(actor.identity_id().to_string())
    .bind(request.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    else {
        return Ok(None);
    };
    if uuid(&r, "project_id")? != project
        || r.try_get::<String, _>("purpose").map_err(storage_error)? != purpose
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != canonical_hash(value)
    {
        return Err(Error::Conflict);
    }
    let raw = cipher::read(
        tx,
        actor.identity_id(),
        project,
        request,
        "request",
        r.try_get("request_size").map_err(storage_error)?,
        keys,
    )
    .await?;
    if serde_json::from_slice::<Value>(&raw).map_err(storage_error)? != *value {
        return Err(Error::Conflict);
    }
    let receipt = cipher::read(
        tx,
        actor.identity_id(),
        project,
        request,
        "receipt",
        r.try_get("receipt_size").map_err(storage_error)?,
        keys,
    )
    .await?;
    let v: AndroidWorkflowOperationReceipt =
        serde_json::from_slice(&receipt).map_err(storage_error)?;
    if canonical_hash(&v)
        != r.try_get::<String, _>("receipt_sha256")
            .map_err(storage_error)?
        || v.owner_id != actor.identity_id()
        || v.project_id != project
        || v.request_id != request
        || v.request_sha256 != canonical_hash(value)
        || v.workflow.id != uuid(&r, "workflow_id")?
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(v))
}
async fn commit_operation(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    request: Uuid,
    purpose: &str,
    body: &Value,
    receipt: &AndroidWorkflowOperationReceipt,
    keys: &dyn KeyStore,
    key: KeyId,
    current: i64,
) -> Result<()> {
    let req = Zeroizing::new(serde_json::to_vec(body).map_err(storage_error)?);
    let out = Zeroizing::new(serde_json::to_vec(receipt).map_err(storage_error)?);
    if req.len() > 3 * 1024 * 1024 || out.len() > 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    quota(tx, owner, req.len() + out.len()).await?;
    cipher::write(tx, owner, project, request, "request", &req, keys, key).await?;
    cipher::write(tx, owner, project, request, "receipt", &out, keys, key).await?;
    sqlx::query("INSERT INTO android_private_workflow_operations(owner_id,request_id,project_id,workflow_id,purpose,request_sha256,request_size,receipt_sha256,receipt_size,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(owner.to_string()).bind(request.to_string()).bind(project.to_string()).bind(receipt.workflow.id.to_string()).bind(purpose).bind(canonical_hash(body)).bind(req.len() as i64).bind(canonical_hash(receipt)).bind(out.len() as i64).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn selected(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    selection: &AndroidWorkflowSelection,
    keys: &dyn KeyStore,
) -> Result<AndroidWorkflowVersion> {
    let r = head(tx, owner, project, selection.workflow_id).await?;
    let e = entry(tx, &r).await?;
    if e.revoked
        || e.revision != selection.registry_revision
        || e.published_version.as_ref().is_none_or(|v| {
            v.id != selection.version_id || v.definition_sha256 != selection.definition_sha256
        })
    {
        return Err(Error::Forbidden);
    }
    let n:i64=sqlx::query_scalar("SELECT definition_size FROM android_private_workflow_versions WHERE id=$1 AND workflow_id=$2 AND owner_id=$3 AND project_id=$4").bind(selection.version_id.to_string()).bind(selection.workflow_id.to_string()).bind(owner.to_string()).bind(project.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let bytes = cipher::read(tx, owner, project, selection.version_id, "version", n, keys).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
    if canonical_hash(&value) != selection.definition_sha256 {
        return Err(Error::Forbidden);
    }
    definition::validate(&e.engine, &value)?;
    Ok(AndroidWorkflowVersion {
        version: version_ref(tx, &r, selection.version_id).await?,
        workflow: e,
        definition: value,
    })
}
pub(crate) async fn selection_for_task(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    config: &Value,
) -> Result<Option<AndroidWorkflowSelection>> {
    let value = config
        .get("runner")
        .or_else(|| config["sync_metadata"].get("runner"))
        .and_then(Value::as_str)
        .unwrap_or("model");
    if value == "model" {
        return Ok(None);
    }
    if !matches!(value, "crew" | "workflow") {
        return Err(device_invalid());
    }
    let cid = config
        .get("crew_id")
        .or_else(|| config["sync_metadata"].get("crew_id"))
        .or_else(|| config.get("workflow_id"))
        .and_then(Value::as_str)
        .ok_or_else(device_invalid)?;
    let raw = Uuid::parse_str(cid).ok();
    let rows=sqlx::query("SELECT * FROM android_private_workflows WHERE owner_id=$1 AND project_id=$2 AND engine=$3 AND revoked=0 AND (id=$4 OR source_entity_id=$4 OR source_legacy_id=$4) LIMIT 2").bind(owner.to_string()).bind(project.to_string()).bind(value).bind(raw.map(|v|v.to_string()).unwrap_or_else(||cid.to_owned())).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() != 1 {
        return Err(Error::Validation(
            "private_workflow_requires_current_publication".into(),
        ));
    }
    let e = entry(tx, &rows[0]).await?;
    let v = e.published_version.ok_or(Error::Forbidden)?;
    if let Some(expected) = config.get("workflow_version_id").and_then(Value::as_str) {
        if expected != v.id.to_string() {
            return Err(Error::Conflict);
        }
    }
    Ok(Some(AndroidWorkflowSelection {
        workflow_id: e.id,
        version_id: v.id,
        definition_sha256: v.definition_sha256,
        registry_revision: e.revision,
    }))
}
impl Store {
    pub async fn android_workflows(
        &self,
        a: &Principal,
        project: Uuid,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<AndroidWorkflowPage> {
        if !(1..=50).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let ids=sqlx::query("SELECT * FROM android_private_workflows WHERE owner_id=$1 AND project_id=$2 AND id>$3 ORDER BY id LIMIT $4").bind(a.identity_id().to_string()).bind(project.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut items = Vec::new();
        for r in ids.iter().take(limit as usize) {
            items.push(entry(&mut tx, r).await?)
        }
        let next_cursor = if ids.len() > limit as usize {
            items.last().map(|e| e.id)
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidWorkflowPage { items, next_cursor })
    }
    pub async fn android_workflow_write(
        &self,
        a: &Principal,
        id: Option<Uuid>,
        q: &AndroidWorkflowWrite,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidWorkflowOperationReceipt> {
        if q.request_id.is_nil()
            || q.expected_revision < 0
            || q.name.trim().is_empty()
            || q.name.len() > 200
            || q.source_entity_id.is_some() != q.source_revision.is_some()
        {
            return Err(device_invalid());
        }
        definition::validate(&q.engine, &q.definition)?;
        let bytes = Zeroizing::new(serde_json::to_vec(&q.definition).map_err(storage_error)?);
        if bytes.len() > MAX_BODY {
            return Err(Error::BudgetExceeded);
        }
        let purpose = if id.is_some() { "version" } else { "create" };
        let body =
            json!({"format":"android-private-workflow-operation/1","workflow_id":id,"request":q});
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        self.android_personal_permit(&mut tx, a, q.project_id, ContentAction::Read)
            .await?;
        if let Some(old) =
            original(&mut tx, a, q.project_id, q.request_id, purpose, &body, keys).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        self.android_personal_permit(&mut tx, a, q.project_id, ContentAction::Execute)
            .await?;
        quota(&mut tx, a.identity_id(), bytes.len()).await?;
        if let Some(source) = q.source_entity_id {
            let r=sqlx::query("SELECT revision,payload FROM android_wire_sync_current WHERE owner_id=$1 AND entity_id=$2 AND entity_type='crew' AND team_id IS NULL AND tombstone=0").bind(a.identity_id().to_string()).bind(source.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
            if r.try_get::<i64, _>("revision").map_err(storage_error)?
                != q.source_revision.ok_or(Error::Forbidden)?
                || parse::<Value>(&r.try_get::<String, _>("payload").map_err(storage_error)?)?
                    .get("definition")
                    != Some(&q.definition)
            {
                return Err(Error::Conflict);
            }
        }
        let wid = id.unwrap_or_else(Uuid::new_v4);
        let old = if id.is_some() {
            Some(head(&mut tx, a.identity_id(), q.project_id, wid).await?)
        } else {
            None
        };
        let rev = if let Some(r) = old {
            if r.try_get::<i64, _>("revision").map_err(storage_error)? != q.expected_revision
                || r.try_get::<String, _>("engine").map_err(storage_error)? != engine(&q.engine)
            {
                return Err(Error::Conflict);
            }
            q.expected_revision
                .checked_add(1)
                .ok_or(Error::BudgetExceeded)?
        } else {
            if q.expected_revision != 0 {
                return Err(Error::Conflict);
            }
            1
        };
        let vid = Uuid::new_v4();
        if id.is_none() {
            sqlx::query("INSERT INTO android_private_workflows(id,owner_id,project_id,name,engine,revision,revoked,published_version_id,source_entity_id,source_revision,source_legacy_id) VALUES($1,$2,$3,$4,$5,1,0,NULL,$6,$7,$8)").bind(wid.to_string()).bind(a.identity_id().to_string()).bind(q.project_id.to_string()).bind(&q.name).bind(engine(&q.engine)).bind(q.source_entity_id.map(|v|v.to_string())).bind(q.source_revision).bind(if q.publish && q.engine==AndroidWorkflowEngine::Crew {q.definition["id"].as_str().map(str::to_owned)}else{None}).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let vrev:i64=sqlx::query_scalar("SELECT COALESCE(MAX(revision),0)+1 FROM android_private_workflow_versions WHERE workflow_id=$1").bind(wid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO android_private_workflow_versions(id,workflow_id,owner_id,project_id,revision,definition_sha256,definition_size,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(vid.to_string()).bind(wid.to_string()).bind(a.identity_id().to_string()).bind(q.project_id.to_string()).bind(vrev).bind(canonical_hash(&q.definition)).bind(bytes.len() as i64).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        cipher::write(
            &mut tx,
            a.identity_id(),
            q.project_id,
            vid,
            "version",
            &bytes,
            keys,
            key,
        )
        .await?;
        let changed=sqlx::query("UPDATE android_private_workflows SET revision=$1,name=$2,published_version_id=CASE WHEN $3=1 THEN $4 ELSE published_version_id END,revoked=CASE WHEN $3=1 THEN 0 ELSE revoked END,source_entity_id=$5,source_revision=$6,source_legacy_id=CASE WHEN $3=1 THEN $11 ELSE source_legacy_id END WHERE id=$7 AND owner_id=$8 AND project_id=$9 AND revision=$10").bind(rev).bind(&q.name).bind(i64::from(q.publish)).bind(vid.to_string()).bind(q.source_entity_id.map(|v|v.to_string())).bind(q.source_revision).bind(wid.to_string()).bind(a.identity_id().to_string()).bind(q.project_id.to_string()).bind(if id.is_none(){1}else{q.expected_revision}).bind(if q.engine==AndroidWorkflowEngine::Crew {q.definition["id"].as_str().map(str::to_owned)}else{None}).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let r = head(&mut tx, a.identity_id(), q.project_id, wid).await?;
        let receipt = AndroidWorkflowOperationReceipt {
            request_id: q.request_id,
            owner_id: a.identity_id(),
            project_id: q.project_id,
            request_sha256: canonical_hash(&body),
            workflow: entry(&mut tx, &r).await?,
            version: Some(version_ref(&mut tx, &r, vid).await?),
            committed_at: device_iso(current)?,
        };
        commit_operation(
            &mut tx,
            a.identity_id(),
            q.project_id,
            q.request_id,
            purpose,
            &body,
            &receipt,
            keys,
            key,
            current,
        )
        .await?;
        self.android_personal_permit(&mut tx, a, q.project_id, ContentAction::Execute)
            .await?;
        device_writer(&mut tx).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_workflow_change(
        &self,
        a: &Principal,
        project: Uuid,
        wid: Uuid,
        q: &AndroidWorkflowChange,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidWorkflowOperationReceipt> {
        if q.request_id.is_nil() || q.expected_revision <= 0 || q.revoke == q.version_id.is_some() {
            return Err(device_invalid());
        }
        let body =
            json!({"format":"android-private-workflow-operation/1","workflow_id":wid,"request":q});
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        if let Some(old) =
            original(&mut tx, a, project, q.request_id, "change", &body, keys).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        self.android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        let r = head(&mut tx, a.identity_id(), project, wid).await?;
        if r.try_get::<i64, _>("revision").map_err(storage_error)? != q.expected_revision {
            return Err(Error::Conflict);
        }
        let mut alias: Option<String> = None;
        if let Some(id) = q.version_id {
            let version = version_ref(&mut tx, &r, id).await?;
            let size:i64=sqlx::query_scalar("SELECT definition_size FROM android_private_workflow_versions WHERE id=$1 AND workflow_id=$2").bind(id.to_string()).bind(wid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let bytes =
                cipher::read(&mut tx, a.identity_id(), project, id, "version", size, keys).await?;
            let original: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
            if canonical_hash(&original) != version.definition_sha256 {
                return Err(Error::Forbidden);
            }
            if r.try_get::<String, _>("engine").map_err(storage_error)? == "crew" {
                alias = Some(definition::legacy_id(&original)?);
            }
        }
        let changed=sqlx::query("UPDATE android_private_workflows SET revision=revision+1,revoked=$1,published_version_id=CASE WHEN $1=1 THEN published_version_id ELSE $2 END,source_legacy_id=CASE WHEN $1=1 THEN source_legacy_id ELSE $7 END WHERE id=$3 AND owner_id=$4 AND project_id=$5 AND revision=$6").bind(i64::from(q.revoke)).bind(q.version_id.map(|v|v.to_string())).bind(wid.to_string()).bind(a.identity_id().to_string()).bind(project.to_string()).bind(q.expected_revision).bind(alias).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let r = head(&mut tx, a.identity_id(), project, wid).await?;
        let receipt = AndroidWorkflowOperationReceipt {
            request_id: q.request_id,
            owner_id: a.identity_id(),
            project_id: project,
            request_sha256: canonical_hash(&body),
            workflow: entry(&mut tx, &r).await?,
            version: match q.version_id {
                Some(v) => Some(version_ref(&mut tx, &r, v).await?),
                None => None,
            },
            committed_at: device_iso(current)?,
        };
        commit_operation(
            &mut tx,
            a.identity_id(),
            project,
            q.request_id,
            "change",
            &body,
            &receipt,
            keys,
            key,
            current,
        )
        .await?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        device_writer(&mut tx).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_workflow_version(
        &self,
        a: &Principal,
        project: Uuid,
        wid: Uuid,
        vid: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<AndroidWorkflowVersion> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let r = head(&mut tx, a.identity_id(), project, wid).await?;
        let vr = version_ref(&mut tx, &r, vid).await?;
        let n:i64=sqlx::query_scalar("SELECT definition_size FROM android_private_workflow_versions WHERE id=$1 AND workflow_id=$2").bind(vid.to_string()).bind(wid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let bytes =
            cipher::read(&mut tx, a.identity_id(), project, vid, "version", n, keys).await?;
        let definition: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
        if canonical_hash(&definition) != vr.definition_sha256 {
            return Err(Error::Forbidden);
        }
        let result = AndroidWorkflowVersion {
            workflow: entry(&mut tx, &r).await?,
            version: vr,
            definition,
        };
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}

pub(crate) async fn device_definition_in_tx(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    config: &Value,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let selection = selection_for_task(tx, owner, project, config)
        .await?
        .ok_or(Error::Forbidden)?;
    let original = selected(tx, owner, project, &selection, keys).await?;
    if original.workflow.engine != AndroidWorkflowEngine::Crew {
        return Err(Error::Forbidden);
    }
    definition::validate(&original.workflow.engine, &original.definition)?;
    Ok(original.definition)
}
