//! Original private-node MCP task coordination; no owner or Team custody.
use super::*;
use crate::mcp_scoped_tasks::{self, Facts};
use crate::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord};
use cowork_mcp::{CallResult, PreparedTaskOperation, Session, SessionCipher};
pub(super) async fn facts(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    lease: Uuid,
    sequence: i64,
    keys: &dyn KeyStore,
) -> Result<Facts> {
    let current = device_writer(tx).await?;
    let r = leased(store, tx, run, lease, current).await?;
    let cp = tools::checkpoint(tx, &r, keys).await?;
    tools::loop_authority(store, tx, &r, &cp, current).await?;
    let step=sqlx::query("SELECT checkpoint_sha,intent,request_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='mcp' AND state='admitted' AND receipt IS NULL").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
    if step
        .try_get::<String, _>("checkpoint_sha")
        .map_err(storage_error)?
        != canonical_hash(&cp)
    {
        return Err(Error::Conflict);
    }
    let intent: Value = serde_json::from_slice(&workspace::open_value(
        keys,
        &r,
        &format!("intent:{sequence}"),
        &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
    )?)
    .map_err(storage_error)?;
    let request_sha: String = step.try_get("request_sha").map_err(storage_error)?;
    let tool = tools::pending(&cp)?;
    let fingerprint = tools::tool_fingerprint(run, &cp, &tool);
    if canonical_hash(&intent) != request_sha
        || intent["fingerprint"] != fingerprint
        || intent["protocol"] != MCP_PROTOCOL_VERSION
        || intent["task"]["ttl"].as_u64().is_none()
        || intent["tool"] != json!(tool)
    {
        return Err(Error::Forbidden);
    }
    let approval = Uuid::parse_str(intent["approval_id"].as_str().ok_or_else(device_invalid)?)
        .map_err(|_| device_invalid())?;
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_requests WHERE id=$1 AND run_id=$2 AND state='approved' AND revision=$3 AND fingerprint=$4 AND expires_at>$5").bind(approval.to_string()).bind(run.to_string()).bind(intent["approval_revision"].as_i64().ok_or_else(device_invalid)?).bind(&fingerprint).bind(current).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if count != 1 {
        return Err(Error::Forbidden);
    }
    let binding: McpToolBinding =
        serde_json::from_value(intent["binding"].clone()).map_err(storage_error)?;
    if intent["transport_endpoint"]
        .as_str()
        .is_some_and(|s| s.starts_with("cowork-isolated-stdio://"))
    {
        let checkpoint:String=sqlx::query_scalar("SELECT checkpoint FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND state='admitted'").bind(run.to_string()).bind(&binding.server_id).fetch_one(&mut **tx).await.map_err(storage_error)?;
        isolated(
            tx,
            &r,
            Uuid::parse_str(&binding.server_id).map_err(|_| device_invalid())?,
            &parse(&checkpoint)?,
        )
        .await?;
    }
    tools::live_after_crypto(store, tx, &r, &cp).await?;
    let _ = store;
    let binding_sha256 = canonical_hash(&binding);
    Ok(Facts {
        owner_kind: "personal".into(),
        owner_id: id(&r, "owner_id")?.to_string(),
        project_id: id(&r, "project_id")?.to_string(),
        run_id: run.to_string(),
        call_id: step_call(&cp, &tool)?,
        server_id: binding.server_id,
        server_revision: binding.server_revision,
        binding_sha256,
        spec_sha256: canonical_hash(&json!([cp["spec"], cp["workflow_node"]])),
        connection_id: intent["connection_id"]
            .as_str()
            .ok_or_else(device_invalid)?
            .into(),
        connection_epoch: intent["epoch"]
            .as_u64()
            .and_then(|e| e.checked_add(1))
            .ok_or_else(device_invalid)?,
        original_rpc_id: intent["rpc_request_id"]
            .as_str()
            .ok_or_else(device_invalid)?
            .into(),
        original_request_sha256: request_sha,
        transport_request_sha256: intent["transport_request_sha256"]
            .as_str()
            .ok_or_else(device_invalid)?
            .into(),
        authority_sha256: canonical_hash(&json!([
            run,
            lease,
            sequence,
            fingerprint,
            cp["workflow_node"],
            intent["approval_id"],
            intent["approval_revision"],
            id(&r, "owner_id")?,
            id(&r, "project_id")?,
            r.try_get::<i64, _>("project_revision")
                .map_err(storage_error)?
        ])),
    })
}
fn step_call(cp: &Value, tool: &ToolCall) -> Result<String> {
    let original = tools::invocation(cp, tool)?;
    if original.len() <= 128 {
        Ok(original)
    } else {
        Ok(format!("task:{}", canonical_hash(&original)))
    }
}
impl Store {
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_task_capture: Keep run/lease/sequence, original session/task operation and native key custody explicit"
    )]
    pub async fn android_private_mcp_task_capture(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        result: &CallResult,
        session: &Session,
        cipher: Option<&SessionCipher>,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let f = facts(self, &mut tx, run, lease, sequence, keys).await?;
        let scope = CheckpointScope {
            project_id: f.project_id.clone(),
            run_id: f.run_id.clone(),
            server_id: f.server_id.clone(),
            connection_record_id: format!("{run}:{}", f.server_id),
            server_revision: f.server_revision,
            checkpoint_revision: 1,
            configuration_sha256: f.binding_sha256.clone(),
            catalog_sha256: session.catalog().ok_or(Error::Forbidden)?.sha256().into(),
        };
        let task = mcp_scoped_tasks::capture(
            &mut tx,
            &f,
            result,
            session,
            cipher.map(|c| (c, scope)),
            keys,
            key,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_task_begin: Keep run/lease/sequence, original session/task operation and native key custody explicit"
    )]
    pub async fn android_private_mcp_task_begin(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        task: &McpTaskRecord,
        prepared: &PreparedTaskOperation,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskOperationPermit> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let f = facts(self, &mut tx, run, lease, sequence, keys).await?;
        let p = mcp_scoped_tasks::begin(&mut tx, &f, task, prepared, keys, key).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(p)
    }
    pub async fn android_private_mcp_task_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let f = facts(self, &mut tx, run, lease, sequence, keys).await?;
        mcp_scoped_tasks::validate(&mut tx, &f, permit, key).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_mcp_task_commit(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let f = facts(self, &mut tx, run, lease, sequence, keys).await?;
        let task = mcp_scoped_tasks::commit(&mut tx, &f, permit, keys, key).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
}
