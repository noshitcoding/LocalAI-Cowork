//! Original native graph tool authority for acknowledged MCP tasks.
use super::*;
use crate::mcp_scoped_tasks::{self, Facts};
use crate::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord};
use cowork_mcp::{CallResult, CheckpointScope, PreparedTaskOperation, Session, SessionCipher};
use cowork_secrets::{KeyId, KeyStore};

pub struct WorkflowMcpContext {
    pub record: RunRecord,
    pub turn: WorkflowModelTurn,
    pub tool: ToolCall,
    pub deadline: i64,
}
/// Held by the original sender; never reconstructed by a task/read API.
pub struct WorkflowMcpSource {
    facts: Facts,
    key: KeyId,
    checkpoint: CheckpointScope,
}
impl WorkflowMcpSource {
    pub fn checkpoint_scope(&self) -> &CheckpointScope {
        &self.checkpoint
    }
}

async fn authority(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    run: &str,
    lease: &str,
    turn_id: &str,
    raw: &str,
    key: KeyId,
) -> Result<(Facts, WorkflowMcpContext)> {
    if !store.is_local_sqlite() {
        return Err(Error::Forbidden);
    }
    crate::mcp_oauth::evidence_cipher(tx, "native", key).await?;
    if sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
        .bind(run)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let parent: StoredRun = parse(
        &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
            .bind(run)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )?;
    parent.assert_lease(lease, now())?;
    if parent.record.spec.project_id != project {
        return Err(Error::Forbidden);
    }
    let observed = store
        .workflow_clock
        .execution_now(tx, &parent.record)
        .await?;
    let workflow = runs::load(tx, &parent.record).await?;
    let deadline = tools::verify_gate(
        tx,
        &parent,
        &workflow,
        &tools::ToolGate::ExternalCheck {
            turn: turn_id.into(),
            raw_id: raw.into(),
        },
    )
    .await?;
    if observed >= deadline {
        return Err(Error::BudgetExceeded);
    }
    let (turn, tool) = tools::proposal(&workflow, turn_id, raw)?;
    let record = tools::scoped_record(&parent.record, &workflow, turn)?;
    let binding = cowork_domain::mcp::mcp_binding(&record, &tool)?;
    crate::mcp_servers::check_binding(tx, &record, binding).await?;
    let row = sqlx::query("SELECT preparation_sha256,payload FROM workflow_external_evidence WHERE run_id=$1 AND tool_call_id=$2")
        .bind(run).bind(&tool.id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let evidence: evidence::WorkflowExternalEvidence =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    evidence.validate()?;
    let p = &evidence.preparation;
    if evidence.run_id != run
        || evidence.turn_id != turn_id
        || evidence.raw_id != raw
        || evidence.tool != tool
        || evidence.kind != "mcp"
        || evidence.result.is_some()
        || evidence.scope_sha256 != canonical_hash(&record.spec)
        || evidence.preparation_sha256
            != row
                .try_get::<String, _>("preparation_sha256")
                .map_err(storage_error)?
        || p["binding"] != json!(binding)
        || p["protocol"] != MCP_PROTOCOL_VERSION
        || p["task_ttl_ms"].as_u64().is_none()
    {
        return Err(Error::Forbidden);
    }
    let text = |name: &str| {
        p[name]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or(Error::Forbidden)
    };
    let facts = Facts {
        owner_kind: "native".into(),
        owner_id: key.instance.to_string(),
        project_id: project.into(),
        run_id: run.into(),
        call_id: tool.id.clone(),
        server_id: binding.server_id.clone(),
        server_revision: binding.server_revision,
        binding_sha256: canonical_hash(binding),
        spec_sha256: canonical_hash(&parent.record.spec),
        connection_id: text("connection_id")?,
        connection_epoch: p["connection_epoch"].as_u64().ok_or(Error::Forbidden)?,
        original_rpc_id: text("rpc_request_id")?,
        original_request_sha256: text("original_request_sha256")?,
        transport_request_sha256: text("request_sha256")?,
        authority_sha256: canonical_hash(&json!([
            "workflow-mcp-task-authority/1",
            key,
            parent.record.spec,
            record.spec,
            tools::execution_claim(&workflow, turn)?,
            tool,
            evidence.preparation_sha256,
            hash(lease.as_bytes())
        ])),
    };
    Ok((
        facts,
        WorkflowMcpContext {
            record,
            turn: turn.clone(),
            tool,
            deadline,
        },
    ))
}

impl Store {
    pub async fn workflow_mcp_task_source(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        key: KeyId,
    ) -> Result<WorkflowMcpSource> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (facts, original) =
            authority(self, &mut tx, project, run, lease, turn, raw, key).await?;
        let binding = cowork_domain::mcp::mcp_binding(&original.record, &original.tool)?;
        let checkpoint = CheckpointScope {
            project_id: project.into(),
            run_id: run.into(),
            server_id: binding.server_id.clone(),
            connection_record_id: original.tool.id.clone(),
            server_revision: binding.server_revision,
            checkpoint_revision: 1,
            configuration_sha256: binding.configuration_sha256.clone(),
            catalog_sha256: binding.catalog_sha256.clone(),
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(WorkflowMcpSource {
            facts,
            key,
            checkpoint,
        })
    }
    pub async fn workflow_mcp_task_context(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        key: KeyId,
    ) -> Result<WorkflowMcpContext> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (_, context) = authority(self, &mut tx, project, run, lease, turn, raw, key).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(context)
    }
    pub async fn capture_workflow_mcp_task(
        &self,
        source: WorkflowMcpSource,
        result: &CallResult,
        session: &Session,
        cipher: Option<(&SessionCipher, CheckpointScope)>,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        if source.key != key {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Preserve the real original ACK before any new role/lease admission.
        // Every following RPC separately checks current graph authority.
        let value =
            mcp_scoped_tasks::capture(&mut tx, &source.facts, result, session, cipher, keys, key)
                .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn begin_workflow_mcp_task_operation(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        task: &McpTaskRecord,
        prepared: &PreparedTaskOperation,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskOperationPermit> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (facts, _) = authority(self, &mut tx, project, run, lease, turn, raw, key).await?;
        let value = mcp_scoped_tasks::begin(&mut tx, &facts, task, prepared, keys, key).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn validate_workflow_mcp_task_operation(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        permit: &McpTaskOperationPermit,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (facts, _) = authority(self, &mut tx, project, run, lease, turn, raw, key).await?;
        mcp_scoped_tasks::validate(&mut tx, &facts, permit, key).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn commit_workflow_mcp_task_operation(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (facts, _) = authority(self, &mut tx, project, run, lease, turn, raw, key).await?;
        let value = mcp_scoped_tasks::commit(&mut tx, &facts, permit, keys, key).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
