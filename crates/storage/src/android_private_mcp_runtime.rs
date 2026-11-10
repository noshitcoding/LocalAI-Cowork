//! Actor-private MCP coordination. Original transport admissions and receipts are never replay permits.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use crate::android_private_server::{event, id, leased, row, save};
use crate::android_private_tools as tools;
use crate::android_private_workspace as workspace;
use cowork_mcp::{Catalog, CheckpointScope, PreparedCall};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};
#[path = "android_private_mcp_sampling.rs"]
mod sampling;
#[path = "android_private_mcp_tasks.rs"]
mod tasks;
pub(crate) use sampling::charge as private_sampling_charge;
#[path = "android_private_mcp_closure.rs"]
mod closure;
pub use closure::PrivateMcpClosure;

pub struct PrivateMcpConnection {
    pub entity: Uuid,
    pub revision: i64,
    pub binding_sha256: String,
    pub catalog_sha256: String,
    pub definitions: Vec<Value>,
    pub protocol: String,
    pub checkpoint: Value,
}
impl PrivateMcpConnection {
    pub fn scope(
        &self,
        project: Uuid,
        run: Uuid,
        server_revision: i64,
        revision: i64,
    ) -> CheckpointScope {
        CheckpointScope {
            project_id: project.to_string(),
            run_id: run.to_string(),
            server_id: self.entity.to_string(),
            connection_record_id: format!("{run}:{}", self.entity),
            server_revision,
            checkpoint_revision: revision,
            configuration_sha256: self.binding_sha256.clone(),
            catalog_sha256: self.catalog_sha256.clone(),
        }
    }
}
pub enum PrivateMcpGate {
    Waiting(Value),
    Denied(Value),
    Ready,
}
pub struct PrivateMcpWorkspaceAdmission {
    pub sequence: i64,
    pub request_sha256: String,
    pub nonce: Uuid,
}
/// Owns the single non-serializable send opportunity; database observations cannot construct it.
pub struct PrivateMcpPermit {
    prepared: PreparedCall,
    pub sequence: i64,
    pub request_sha256: String,
    pub entity: Uuid,
    pub connection_revision: i64,
}
impl PrivateMcpPermit {
    pub fn into_prepared(self) -> PreparedCall {
        self.prepared
    }
}

async fn connection(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    entity: Uuid,
    keys: &dyn KeyStore,
) -> Result<Option<PrivateMcpConnection>> {
    let Some(c) =
        sqlx::query("SELECT * FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2")
            .bind(id(r, "id")?.to_string())
            .bind(entity.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
    else {
        return Ok(None);
    };
    if c.try_get::<String, _>("state").map_err(storage_error)? == "closed" {
        closure::verified(tx, r, &c, keys).await?;
        closure::quiescent(tx, id(r, "id")?).await?;
        return Ok(None);
    }
    if c.try_get::<String, _>("state").map_err(storage_error)? != "ready" {
        return Err(Error::UncertainEffect);
    }
    let bytes = workspace::open_value(
        keys,
        r,
        &format!("mcp-catalog:{entity}"),
        &parse(
            &c.try_get::<String, _>("definitions")
                .map_err(storage_error)?,
        )?,
    )?;
    let definitions: Vec<Value> = serde_json::from_slice(&bytes).map_err(storage_error)?;
    let catalog = Catalog::from_definitions(definitions.clone()).map_err(|_| Error::Forbidden)?;
    let catalog_sha256: String = c.try_get("catalog_sha").map_err(storage_error)?;
    if catalog.sha256() != catalog_sha256 {
        return Err(Error::Forbidden);
    }
    let checkpoint: Value = parse(
        &c.try_get::<Option<String>, _>("checkpoint")
            .map_err(storage_error)?
            .ok_or(Error::UncertainEffect)?,
    )?;
    if checkpoint["connection_id"]
        .as_str()
        .is_none_or(|s| Uuid::parse_str(s).is_err())
        || checkpoint["epoch"].as_u64().is_none()
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(PrivateMcpConnection {
        entity,
        revision: c.try_get("revision").map_err(storage_error)?,
        binding_sha256: c.try_get("binding_sha").map_err(storage_error)?,
        catalog_sha256,
        definitions,
        protocol: c.try_get("protocol").map_err(storage_error)?,
        checkpoint,
    }))
}
async fn frozen(tx: &mut Transaction<'_, Any>, r: &AnyRow, entity: Uuid) -> Result<Value> {
    let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    crate::android_private_mcp::validate_bindings(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        &v["spec"]["input"],
    )
    .await?;
    v["spec"]["input"]["frozen_mcp_bindings"]
        .as_array()
        .and_then(|a| a.iter().find(|f| f["entity_id"] == entity.to_string()))
        .cloned()
        .ok_or(Error::Forbidden)
}
fn binding<'a>(spec: &'a RunSpec, name: &str) -> Result<&'a McpToolBinding> {
    let mut all = spec.mcp_tools.iter().filter(|b| b.alias == name);
    let value = all.next().ok_or(Error::Forbidden)?;
    if all.next().is_some()
        || !cowork_domain::mcp::valid_binding(value)
        || !spec.allowed_tools.contains(&value.alias)
    {
        return Err(Error::Forbidden);
    }
    Ok(value)
}
pub(crate) async fn validate_spec(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    spec: &RunSpec,
    keys: &dyn KeyStore,
) -> Result<()> {
    if spec.mcp_tools.len() > 128 {
        return Err(Error::BudgetExceeded);
    }
    let mut aliases = std::collections::BTreeSet::new();
    for b in &spec.mcp_tools {
        if !cowork_domain::mcp::valid_binding(b) || !aliases.insert(&b.alias) {
            return Err(Error::Forbidden);
        }
        let entity = Uuid::parse_str(&b.server_id).map_err(|_| device_invalid())?;
        let selected = frozen(tx, r, entity).await?;
        let c = connection(tx, r, entity, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        if selected["revision"] != b.server_revision
            || selected["binding_sha256"] != b.configuration_sha256
            || c.binding_sha256 != b.configuration_sha256
            || c.catalog_sha256 != b.catalog_sha256
            || !c.definitions.contains(&b.definition)
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "add_admission: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
)]
async fn add_admission(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    invocation: &str,
    cp_sha: &str,
    intent: &Value,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<i64> {
    let key_id = workspace::writer_cipher(keys, key_id)?;
    let sequence = tools::next_sequence(tx, id(r, "id")?).await?;
    let sealed = workspace::seal_value(
        keys,
        key_id,
        r,
        &format!("intent:{sequence}"),
        json_string(intent)?.as_bytes(),
    )?;
    crate::server_keys::cipher_writer(tx, key_id).await?;
    sqlx::query("INSERT INTO android_private_loop_steps(run_id,sequence,invocation_id,kind,lease_id,checkpoint_sha,request_sha,intent,state,created_at) VALUES($1,$2,$3,'mcp',$4,$5,$6,$7,'admitted',$8)")
        .bind(id(r,"id")?.to_string()).bind(sequence).bind(invocation).bind(r.try_get::<Option<String>,_>("lease_id").map_err(storage_error)?.ok_or(Error::Forbidden)?).bind(cp_sha).bind(canonical_hash(intent)).bind(json_string(&sealed)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(sequence)
}
#[expect(
    clippy::too_many_arguments,
    reason = "retained_receive: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
)]
async fn retained_receive(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    lease: Uuid,
    sequence: i64,
    request_sha: &str,
    received: &Value,
    confirmed: bool,
    keys: &dyn KeyStore,
    key_id: KeyId,
) -> Result<()> {
    let key_id = workspace::writer_cipher(keys, key_id)?;
    let step=sqlx::query("SELECT request_sha,receipt_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp'").bind(id(r,"id")?.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if step
        .try_get::<String, _>("request_sha")
        .map_err(storage_error)?
        != request_sha
    {
        return Err(Error::Forbidden);
    }
    let digest = canonical_hash(received);
    if let Some(old) = step
        .try_get::<Option<String>, _>("receipt_sha")
        .map_err(storage_error)?
    {
        if old != digest {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    let bytes = json_string(received)?;
    if bytes.len()
        > if received["kind"] == "workspace" {
            24 * 1024 * 1024
        } else {
            4 * 1024 * 1024
        }
    {
        return Err(Error::BudgetExceeded);
    }
    let sealed = workspace::seal_value(
        keys,
        key_id,
        r,
        &format!("receipt:{sequence}"),
        bytes.as_bytes(),
    )?;
    crate::server_keys::cipher_writer(tx, key_id).await?;
    let changed=sqlx::query("UPDATE android_private_loop_steps SET receipt=$1,receipt_sha=$2,state=$3 WHERE run_id=$4 AND sequence=$5 AND receipt IS NULL").bind(json_string(&sealed)?).bind(digest).bind(if confirmed{"received"}else{"uncertain"}).bind(id(r,"id")?.to_string()).bind(sequence).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    if changed != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
impl Store {
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_workspace_admit: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
    )]
    pub async fn android_private_mcp_workspace_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        call_sequence: i64,
        control_id: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PrivateMcpWorkspaceAdmission> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let original=sqlx::query("SELECT intent,receipt_sha,checkpoint_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp' AND state='received'").bind(run.to_string()).bind(call_sequence).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let call: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{call_sequence}"),
            &parse(
                &original
                    .try_get::<String, _>("intent")
                    .map_err(storage_error)?,
            )?,
        )?)
        .map_err(storage_error)?;
        if call["binding"]["server_id"] != entity.to_string()
            || original
                .try_get::<String, _>("checkpoint_sha")
                .map_err(storage_error)?
                != canonical_hash(&cp)
            || call["fingerprint"] != tools::tool_fingerprint(run, &cp, &tools::pending(&cp)?)
        {
            return Err(Error::Forbidden);
        }
        let raw:String=sqlx::query_scalar("SELECT checkpoint FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND state='admitted'").bind(run.to_string()).bind(entity.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let continuation: Value = parse(&raw)?;
        let nonce = isolated(&mut tx, &r, entity, &continuation).await?;
        let invocation = format!("mcp-workspace:{call_sequence}");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2",
        )
        .bind(run.to_string())
        .bind(&invocation)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count != 0 {
            return Err(Error::UncertainEffect);
        }
        if control_id.is_nil() {
            return Err(Error::Forbidden);
        }
        let intent = json!({"format":1,"operation":"capture_private_workspace","entity":entity,"nonce":nonce,"control_id":control_id,"call_sequence":call_sequence,"original_response_sha256":original.try_get::<String,_>("receipt_sha").map_err(storage_error)?,"checkpoint_sha256":canonical_hash(&cp),"isolated":continuation["isolated"]});
        let sequence = add_admission(
            &mut tx,
            &r,
            &invocation,
            &canonical_hash(&cp),
            &intent,
            keys,
            key_id,
            now,
        )
        .await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateMcpWorkspaceAdmission {
            sequence,
            request_sha256: canonical_hash(&intent),
            nonce,
        })
    }
    pub async fn android_private_mcp_workspace_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let step=sqlx::query("SELECT intent,request_sha,checkpoint_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND state='admitted' AND receipt IS NULL AND kind='mcp'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        if intent["operation"] != "capture_private_workspace"
            || canonical_hash(&intent)
                != step
                    .try_get::<String, _>("request_sha")
                    .map_err(storage_error)?
            || step
                .try_get::<String, _>("checkpoint_sha")
                .map_err(storage_error)?
                != canonical_hash(&cp)
        {
            return Err(Error::Forbidden);
        }
        let entity = crate::android_personal::wire_uuid(&intent, "entity")?;
        isolated(
            &mut tx,
            &r,
            entity,
            &json!({"transport":"stdio","isolated":intent["isolated"]}),
        )
        .await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// Receipt custody deliberately survives lease/session revocation; it never authorizes another send.
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_retain_received: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
    )]
    pub async fn android_private_mcp_retain_received(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        request_sha: &str,
        received: &Value,
        confirmed: bool,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        retained_receive(
            &mut tx,
            &r,
            lease,
            sequence,
            request_sha,
            received,
            confirmed,
            keys,
            key_id,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_mcp_connection(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Option<PrivateMcpConnection>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, current).await?;
        let selected = frozen(&mut tx, &r, entity).await?;
        let c = connection(&mut tx, &r, entity, keys).await?;
        if c.as_ref()
            .is_some_and(|c| selected["binding_sha256"] != c.binding_sha256)
        {
            return Err(Error::Forbidden);
        }
        let current = device_writer(&mut tx).await?;
        leased(self, &mut tx, run, lease, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(c)
    }
    /// One original initialize/discovery admission. A lost connection cannot silently be replaced.
    pub async fn android_private_mcp_discovery_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<(i64, String, i64)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, current).await?;
        let selected = frozen(&mut tx, &r, entity).await?;
        let previous = sqlx::query(
            "SELECT * FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2",
        )
        .bind(run.to_string())
        .bind(entity.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let revision = if let Some(previous) = previous.as_ref() {
            if previous
                .try_get::<String, _>("state")
                .map_err(storage_error)?
                != "closed"
            {
                return Err(Error::UncertainEffect);
            }
            closure::verified(&mut tx, &r, previous, keys).await?;
            closure::quiescent(&mut tx, run).await?;
            previous
                .try_get::<i64, _>("revision")
                .map_err(storage_error)?
                .checked_add(1)
                .ok_or(Error::BudgetExceeded)?
        } else {
            1
        };
        let thread_revision:i64=sqlx::query_scalar("SELECT revision FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id(&r,"thread_id")?.to_string()).bind(id(&r,"owner_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let mut intent = json!({"format":1,"operation":"connect_and_discover","run":run,"entity":entity,"frozen_binding":selected,"thread_revision":thread_revision});
        if revision != 1 {
            intent["session_revision"] = json!(revision);
        }
        let sequence = add_admission(
            &mut tx,
            &r,
            &if revision == 1 {
                format!("mcp-discovery:{entity}")
            } else {
                format!("mcp-discovery:{entity}:{revision}")
            },
            &canonical_hash(&intent),
            &intent,
            keys,
            key_id,
            current,
        )
        .await?;
        if let Some(previous) = previous {
            if sqlx::query("UPDATE android_private_mcp_sessions SET binding_sha=$1,catalog_sha='',definitions='{}',protocol='',checkpoint=NULL,revision=$2,state='admitted' WHERE run_id=$3 AND entity_id=$4 AND revision=$5 AND state='closed'").bind(selected["binding_sha256"].as_str().ok_or_else(device_invalid)?).bind(revision).bind(run.to_string()).bind(entity.to_string()).bind(previous.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        } else {
            sqlx::query("INSERT INTO android_private_mcp_sessions(run_id,entity_id,binding_sha,catalog_sha,definitions,protocol,checkpoint,revision,state) VALUES($1,$2,$3,'','{}','',NULL,1,'admitted')").bind(run.to_string()).bind(entity.to_string()).bind(selected["binding_sha256"].as_str().ok_or_else(device_invalid)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let now = device_writer(&mut tx).await?;
        leased(self, &mut tx, run, lease, now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((sequence, canonical_hash(&intent), revision))
    }
    pub async fn android_private_mcp_discovery_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let step=sqlx::query("SELECT intent,request_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp' AND state IN('admitted','received')").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        let revision:i64=sqlx::query_scalar("SELECT revision FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id(&r,"thread_id")?.to_string()).bind(id(&r,"owner_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if intent["operation"] != "connect_and_discover"
            || intent["thread_revision"] != revision
            || canonical_hash(&intent)
                != step
                    .try_get::<String, _>("request_sha")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let now = device_writer(&mut tx).await?;
        leased(self, &mut tx, run, lease, now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_discovery_received: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
    )]
    pub async fn android_private_mcp_discovery_received(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        sequence: i64,
        request_sha: &str,
        received: &Value,
        continuation: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        retained_receive(
            &mut tx,
            &r,
            lease,
            sequence,
            request_sha,
            received,
            true,
            keys,
            key_id,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        // Preserve received bytes even when publication fails or current authority was revoked.
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let selected = frozen(&mut tx, &r, entity).await?;
        let original=sqlx::query("SELECT intent FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(
                &original
                    .try_get::<String, _>("intent")
                    .map_err(storage_error)?,
            )?,
        )?)
        .map_err(storage_error)?;
        let thread_revision:i64=sqlx::query_scalar("SELECT revision FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id(&r,"thread_id")?.to_string()).bind(id(&r,"owner_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if intent["entity"] != entity.to_string()
            || intent["frozen_binding"] != selected
            || intent["thread_revision"] != thread_revision
            || canonical_hash(&intent) != request_sha
        {
            return Err(Error::Forbidden);
        }
        let definitions: Vec<Value> = serde_json::from_value(received["definitions"].clone())
            .map_err(|_| device_invalid())?;
        let catalog = Catalog::from_definitions(definitions).map_err(|_| Error::Forbidden)?;
        if received["catalog_sha256"] != catalog.sha256()
            || received["connection_id"] != continuation["connection_id"]
            || received["epoch"] != continuation["epoch"]
            || !matches!(
                received["protocol"].as_str(),
                Some("2025-11-25" | "2024-11-05")
            )
        {
            return Err(Error::Forbidden);
        }
        if continuation["transport"] != "stdio"
            && (received["protocol"] == "2024-11-05") != (continuation["transport"] == "sse")
        {
            return Err(Error::Forbidden);
        }
        if continuation["transport"] == "stdio" {
            isolated(&mut tx, &r, entity, continuation).await?;
        }
        let original_revision = intent["session_revision"].as_i64().unwrap_or(1);
        if original_revision <= 0 {
            return Err(Error::Forbidden);
        }
        let next_revision = original_revision
            .checked_add(1)
            .ok_or(Error::BudgetExceeded)?;
        validate_continuation(
            run,
            id(&r, "project_id")?,
            entity,
            &selected,
            catalog.sha256(),
            next_revision,
            continuation,
        )?;
        guard_continuation(&mut tx, continuation).await?;
        let sealed = workspace::seal_value(
            keys,
            key_id,
            &r,
            &format!("mcp-catalog:{entity}"),
            json_string(&received["definitions"])?.as_bytes(),
        )?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        let changed=sqlx::query("UPDATE android_private_mcp_sessions SET catalog_sha=$1,definitions=$2,protocol=$3,checkpoint=$4,revision=$8,state='ready' WHERE run_id=$5 AND entity_id=$6 AND revision=$9 AND state='admitted' AND binding_sha=$7")
            .bind(catalog.sha256()).bind(json_string(&sealed)?).bind(received["protocol"].as_str().ok_or_else(device_invalid)?).bind(json_string(continuation)?).bind(run.to_string()).bind(entity.to_string()).bind(selected["binding_sha256"].as_str().ok_or_else(device_invalid)?).bind(next_revision).bind(original_revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let now = device_writer(&mut tx).await?;
        leased(self, &mut tx, run, lease, now).await?;
        event(
            &mut tx,
            run,
            "mcp_catalog_ready",
            &json!({"entity_id":entity,"catalog_sha256":catalog.sha256()}),
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// All remote tools require an exact user approval, regardless of untrusted annotations.
    pub async fn android_private_mcp_gate(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PrivateMcpGate> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let b = binding(&spec, &tool.name)?;
        let entity = Uuid::parse_str(&b.server_id).map_err(|_| device_invalid())?;
        let c = connection(&mut tx, &r, entity, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        if c.binding_sha256 != b.configuration_sha256 || c.catalog_sha256 != b.catalog_sha256 {
            return Err(Error::Forbidden);
        }
        Catalog::from_definitions(c.definitions)
            .map_err(|_| Error::Forbidden)?
            .validate_arguments(
                b.definition["name"].as_str().ok_or_else(device_invalid)?,
                &tool.arguments,
            )
            .map_err(|_| device_invalid())?;
        if cp["model_steps"].as_u64().ok_or_else(device_invalid)?
            + cp["tool_steps"].as_u64().ok_or_else(device_invalid)?
            >= u64::from(spec.budgets.max_steps)
            || cp["total_tokens"].as_u64().ok_or_else(device_invalid)?
                > spec.budgets.max_total_tokens
        {
            return Err(Error::BudgetExceeded);
        }
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        if chrono::DateTime::parse_from_rfc3339(
            req["expires_at"].as_str().ok_or_else(device_invalid)?,
        )
        .map_err(|_| device_invalid())?
        .timestamp_millis()
            <= now
        {
            return Err(Error::Forbidden);
        }
        let progress = match req["state"].as_str() {
            Some("pending") => PrivateMcpGate::Waiting(req),
            Some("approved") => PrivateMcpGate::Ready,
            Some("rejected") => PrivateMcpGate::Denied(
                tools::complete_tool(
                    &mut tx,
                    &r,
                    &mut cp,
                    &tool,
                    &json!({"denied":true,"request_id":req["id"]}),
                    keys,
                    key_id,
                    now,
                )
                .await?,
            ),
            _ => return Err(Error::Conflict),
        };
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(progress)
    }
    pub async fn android_private_mcp_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        prepared: PreparedCall,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PrivateMcpPermit> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let b = binding(&spec, &tool.name)?;
        let entity = Uuid::parse_str(&b.server_id).map_err(|_| device_invalid())?;
        let selected = frozen(&mut tx, &r, entity).await?;
        let c = connection(&mut tx, &r, entity, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        if c.catalog_sha256 != b.catalog_sha256
            || c.binding_sha256 != b.configuration_sha256
            || prepared.connection_id()
                != c.checkpoint["connection_id"]
                    .as_str()
                    .ok_or_else(device_invalid)?
            || prepared.epoch() != c.checkpoint["epoch"].as_u64().ok_or_else(device_invalid)?
            || prepared.catalog_sha256() != b.catalog_sha256
            || prepared.tool_name() != b.definition["name"].as_str().ok_or_else(device_invalid)?
            || prepared.arguments() != &tool.arguments
            || selected["binding_sha256"] != c.binding_sha256
            || (c.checkpoint["transport"] == "stdio"
                && (b.endpoint != prepared.endpoint()
                    || prepared.endpoint()
                        != format!(
                            "cowork-isolated-stdio://{}",
                            c.checkpoint["isolated"]["nonce"]
                                .as_str()
                                .ok_or_else(device_invalid)?
                        )))
        {
            return Err(Error::Forbidden);
        }
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        if req["state"] != "approved"
            || chrono::DateTime::parse_from_rfc3339(
                req["expires_at"].as_str().ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?
            .timestamp_millis()
                <= now
        {
            return Err(Error::Forbidden);
        }
        let mut intent = json!({"format":1,"operation":"tools/call","tool":tool,"fingerprint":tools::tool_fingerprint(run,&cp,&tool),"binding":b,"session_revision":c.revision,"connection_id":prepared.connection_id(),"epoch":prepared.epoch(),"protocol":c.protocol,"rpc_request_id":prepared.rpc_request_id(),"transport_endpoint":prepared.endpoint(),"transport_request_sha256":prepared.request_sha256(),"approval_id":req["id"],"approval_revision":req["revision"]});
        if let Some(ttl) = prepared.task_ttl_ms() {
            intent["task"] = json!({"ttl":ttl});
        }
        if c.checkpoint["transport"] == "stdio" {
            isolated(&mut tx, &r, entity, &c.checkpoint).await?;
        }
        let sequence = add_admission(
            &mut tx,
            &r,
            &tools::invocation(&cp, &tool)?,
            &canonical_hash(&cp),
            &intent,
            keys,
            key_id,
            now,
        )
        .await?;
        let changed=sqlx::query("UPDATE android_private_mcp_sessions SET state='admitted' WHERE run_id=$1 AND entity_id=$2 AND revision=$3 AND state='ready'").bind(run.to_string()).bind(entity.to_string()).bind(c.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "running" {
            save(&mut tx, &r, "running", None, None, now).await?;
        }
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        event(
            &mut tx,
            run,
            "mcp_tool_admitted",
            &json!({"sequence":sequence,"tool_call_id":tool.id,"name":tool.name}),
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateMcpPermit {
            prepared,
            sequence,
            request_sha256: canonical_hash(&intent),
            entity,
            connection_revision: c.revision,
        })
    }
    pub async fn android_private_mcp_validate_dispatch(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let s=sqlx::query("SELECT * FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp' AND state='admitted' AND receipt IS NULL").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        if s.try_get::<String, _>("checkpoint_sha")
            .map_err(storage_error)?
            != canonical_hash(&cp)
        {
            return Err(Error::Conflict);
        }
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&s.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        if canonical_hash(&intent)
            != s.try_get::<String, _>("request_sha")
                .map_err(storage_error)?
            || intent["fingerprint"] != tools::tool_fingerprint(run, &cp, &tools::pending(&cp)?)
        {
            return Err(Error::Forbidden);
        }
        let request = Uuid::parse_str(intent["approval_id"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_requests WHERE id=$1 AND run_id=$2 AND state='approved' AND revision=$3 AND fingerprint=$4 AND expires_at>$5").bind(request.to_string()).bind(run.to_string()).bind(intent["approval_revision"].as_i64().ok_or_else(device_invalid)?).bind(tools::tool_fingerprint(run,&cp,&tools::pending(&cp)?)).bind(now).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        if intent["transport_endpoint"]
            .as_str()
            .is_some_and(|s| s.starts_with("cowork-isolated-stdio://"))
        {
            let raw:String=sqlx::query_scalar("SELECT checkpoint FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND state='admitted'").bind(run.to_string()).bind(intent["binding"]["server_id"].as_str().ok_or_else(device_invalid)?).fetch_one(&mut *tx).await.map_err(storage_error)?;
            isolated(
                &mut tx,
                &r,
                crate::android_personal::wire_uuid(&intent["binding"], "server_id")?,
                &parse(&raw)?,
            )
            .await?;
        }
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_call_received: Keep live run/lease, original admission/receive sequence and encrypted MCP continuation custody explicit"
    )]
    pub async fn android_private_mcp_call_received(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        sequence: i64,
        request_sha: &str,
        received: &Value,
        continuation: Option<&Value>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let confirmed = received["uncertain"] != true;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        retained_receive(
            &mut tx,
            &r,
            lease,
            sequence,
            request_sha,
            received,
            confirmed,
            keys,
            key_id,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        if !confirmed {
            return Err(Error::UncertainEffect);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let step=sqlx::query("SELECT checkpoint_sha,intent FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp' AND state='received'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let previous = canonical_hash(&cp);
        if step
            .try_get::<String, _>("checkpoint_sha")
            .map_err(storage_error)?
            != previous
        {
            return Err(Error::UncertainEffect);
        }
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        let tool = tools::pending(&cp)?;
        if intent["fingerprint"] != tools::tool_fingerprint(run, &cp, &tool)
            || intent["tool"] != json!(tool)
            || received["response_sha256"] != canonical_hash(&received["response"])
        {
            return Err(Error::Forbidden);
        }
        let selected = frozen(&mut tx, &r, entity).await?;
        let revision = intent["session_revision"]
            .as_i64()
            .ok_or_else(device_invalid)?;
        let raw=sqlx::query("SELECT definitions FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND revision=$3 AND state='admitted'").bind(run.to_string()).bind(entity.to_string()).bind(revision).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let definitions: Vec<Value> = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("mcp-catalog:{entity}"),
            &parse(
                &raw.try_get::<String, _>("definitions")
                    .map_err(storage_error)?,
            )?,
        )?)
        .map_err(storage_error)?;
        let catalog = Catalog::from_definitions(definitions).map_err(|_| Error::Forbidden)?;
        catalog
            .validate_result(
                intent["binding"]["definition"]["name"]
                    .as_str()
                    .ok_or_else(device_invalid)?,
                &received["response"],
            )
            .map_err(|_| Error::UncertainEffect)?;
        let next = revision.checked_add(1).ok_or(Error::Conflict)?;
        if intent["transport_endpoint"]
            .as_str()
            .is_some_and(|s| s.starts_with("cowork-isolated-stdio://"))
        {
            let captured=sqlx::query("SELECT sequence,intent,receipt,receipt_sha FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2 AND lease_id=$3 AND kind='mcp' AND state='received'").bind(run.to_string()).bind(format!("mcp-workspace:{sequence}")).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
            let n = captured
                .try_get::<i64, _>("sequence")
                .map_err(storage_error)?;
            let capture_intent: Value = serde_json::from_slice(&workspace::open_value(
                keys,
                &r,
                &format!("intent:{n}"),
                &parse(
                    &captured
                        .try_get::<String, _>("intent")
                        .map_err(storage_error)?,
                )?,
            )?)
            .map_err(storage_error)?;
            let capture: Value = serde_json::from_slice(&workspace::open_value(
                keys,
                &r,
                &format!("receipt:{n}"),
                &parse(
                    &captured
                        .try_get::<String, _>("receipt")
                        .map_err(storage_error)?,
                )?,
            )?)
            .map_err(storage_error)?;
            if capture_intent["call_sequence"] != sequence
                || capture["id"] != capture_intent["control_id"]
                || capture_intent["entity"] != entity.to_string()
                || capture_intent["checkpoint_sha256"] != previous
                || capture_intent["original_response_sha256"] != canonical_hash(received)
                || capture["kind"] != "workspace"
                || capture["nonce"] != capture_intent["nonce"]
                || capture["operation"] != "capture"
                || capture["paused"] != true
                || canonical_hash(&capture)
                    != captured
                        .try_get::<String, _>("receipt_sha")
                        .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            isolated(
                &mut tx,
                &r,
                entity,
                &json!({"transport":"stdio","isolated":capture_intent["isolated"]}),
            )
            .await?;
            crate::android_private_runner::promote(
                &mut tx,
                &r,
                &capture["files"],
                keys,
                key_id,
                now,
            )
            .await?;
        }
        let changed = if let Some(continuation) = continuation {
            if continuation["transport"] == "stdio" {
                isolated(&mut tx, &r, entity, continuation).await?;
            }
            validate_continuation(
                run,
                id(&r, "project_id")?,
                entity,
                &selected,
                catalog.sha256(),
                next,
                continuation,
            )?;
            guard_continuation(&mut tx, continuation).await?;
            if continuation["connection_id"] != intent["connection_id"]
                || continuation["epoch"].as_u64()
                    != intent["epoch"].as_u64().and_then(|v| v.checked_add(1))
            {
                return Err(Error::Forbidden);
            }
            sqlx::query("UPDATE android_private_mcp_sessions SET checkpoint=$1,revision=$2,state='ready' WHERE run_id=$3 AND entity_id=$4 AND revision=$5 AND state='admitted'").bind(json_string(continuation)?).bind(next).bind(run.to_string()).bind(entity.to_string()).bind(revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        } else {
            sqlx::query("UPDATE android_private_mcp_sessions SET checkpoint=NULL,revision=$1,state='closed' WHERE run_id=$2 AND entity_id=$3 AND revision=$4 AND state='admitted'").bind(next).bind(run.to_string()).bind(entity.to_string()).bind(revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        };
        if changed != 1 {
            return Err(Error::Conflict);
        }
        cp["pending_tools"]
            .as_array_mut()
            .ok_or_else(device_invalid)?
            .remove(0);
        cp["tool_steps"] = json!(cp["tool_steps"].as_u64().ok_or_else(device_invalid)? + 1);
        cp["messages"].as_array_mut().ok_or_else(device_invalid)?.push(json!({"role":"tool","tool_call_id":tool.id,"name":tool.name,"content":json_string(&json!({"response":received["response"],"catalog_changed":received["catalog_changed"]}))?}));
        tools::set_checkpoint(&mut tx, &r, &cp, Some(&previous), keys, key_id, now).await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        event(&mut tx,run,"mcp_tool_completed",&json!({"sequence":sequence,"tool_call_id":tool.id,"receipt_sha256":canonical_hash(received)}),now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
async fn guard_continuation(tx: &mut Transaction<'_, Any>, value: &Value) -> Result<()> {
    if value["transport"] == "streamable_http" {
        let sealed: cowork_mcp::SealedSession =
            serde_json::from_value(value["sealed"].clone()).map_err(|_| device_invalid())?;
        let key =
            KeyId::from_cipher_id(sealed.key_id()).map_err(|_| Error::NativeKeyUnavailable)?;
        crate::server_keys::cipher_writer(tx, key).await?;
    }
    Ok(())
}
async fn isolated(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    entity: Uuid,
    value: &Value,
) -> Result<Uuid> {
    if value["transport"] != "stdio" {
        return Err(Error::Forbidden);
    }
    let nonce = crate::android_personal::wire_uuid(&value["isolated"], "nonce")?;
    let record=sqlx::query("SELECT container_id,original_binding,state FROM android_private_containers WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND project_id=$4 AND lease_id=$5 AND invocation_id=$6").bind(nonce.to_string()).bind(id(r,"id")?.to_string()).bind(id(r,"owner_id")?.to_string()).bind(id(r,"project_id")?.to_string()).bind(r.try_get::<Option<String>,_>("lease_id").map_err(storage_error)?.ok_or(Error::Forbidden)?).bind(format!("stdio:{entity}")).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let binding: Value = parse(
        &record
            .try_get::<String, _>("original_binding")
            .map_err(storage_error)?,
    )?;
    let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if !matches!(
        record
            .try_get::<String, _>("state")
            .map_err(storage_error)?
            .as_str(),
        "start_admitted" | "running"
    ) || binding != original["spec"]["input"]["frozen_private_runner"]
        || value["isolated"]["binding_sha256"] != canonical_hash(&binding)
        || value["isolated"]["container_id"]
            != json!(
                record
                    .try_get::<Option<String>, _>("container_id")
                    .map_err(storage_error)?
            )
    {
        return Err(Error::Forbidden);
    }
    Ok(nonce)
}
fn validate_continuation(
    run: Uuid,
    project: Uuid,
    entity: Uuid,
    selected: &Value,
    catalog: &str,
    revision: i64,
    value: &Value,
) -> Result<()> {
    if value["connection_id"]
        .as_str()
        .is_none_or(|s| Uuid::parse_str(s).is_err())
        || value["epoch"].as_u64().is_none()
    {
        return Err(Error::Forbidden);
    }
    match value["transport"].as_str() {
        Some("sse" | "stdio") if value["sealed"].is_null() => Ok(()),
        Some("streamable_http") => {
            let expected = CheckpointScope {
                project_id: project.to_string(),
                run_id: run.to_string(),
                server_id: entity.to_string(),
                connection_record_id: format!("{run}:{entity}"),
                server_revision: selected["revision"].as_i64().ok_or_else(device_invalid)?,
                checkpoint_revision: revision,
                configuration_sha256: selected["binding_sha256"]
                    .as_str()
                    .ok_or_else(device_invalid)?
                    .into(),
                catalog_sha256: catalog.into(),
            };
            if value["scope"] != json!(expected) {
                return Err(Error::Forbidden);
            }
            let sealed: cowork_mcp::SealedSession =
                serde_json::from_value(value["sealed"].clone()).map_err(|_| device_invalid())?;
            sealed.validate().map_err(|_| Error::Forbidden)
        }
        _ => Err(Error::Forbidden),
    }
}
