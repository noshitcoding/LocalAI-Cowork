//! An acknowledged task continues its original tool. It never authorizes a
//! replacement tool call or adopts a task ID supplied by a user/model.
use super::*;
use cowork_domain::mcp::StoredMcpAttempt;
use cowork_mcp::{
    PreparedTaskOperation, Session, SessionCipher, TaskAcknowledgement, TaskOperation,
};
use cowork_secrets::{
    KeyId, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, seal_mcp_blob,
};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::Zeroizing;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpTaskRecord {
    pub view: McpAsyncTask,
    pub owner_kind: String,
    pub owner_id: String,
    pub project_id: String,
    pub server_revision: i64,
    pub binding_sha256: String,
    pub spec_sha256: String,
    pub connection_id: String,
    pub connection_epoch: u64,
    pub original_rpc_id: String,
    pub original_request_sha256: String,
    pub transport_request_sha256: String,
    pub task_id_sha256: String,
    pub scope: McpSecretScope,
    pub sealed_ack: SealedMcpBlob,
    pub sealed_checkpoint: Option<SealedMcpBlob>,
    pub pending_operation_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpTaskSecret {
    pub original: TaskAcknowledgement,
    pub current: TaskAcknowledgement,
    pub original_rpc_response: Value,
}
pub struct McpTaskOperationPermit {
    pub id: String,
    pub task: McpTaskRecord,
    pub operation: TaskOperation,
    pub request_sha256: String,
    pub rpc_request_id: String,
    pub authority_sha256: String,
    pub scope: McpSecretScope,
}
pub(crate) fn result_state(ack: &TaskAcknowledgement, response: &Value) -> Result<&'static str> {
    let body = &response["response"];
    let paths = [
        "/_meta/io.modelcontextprotocol~1related-task/taskId",
        "/error/data/_meta/io.modelcontextprotocol~1related-task/taskId",
    ];
    let ids = paths
        .into_iter()
        .filter_map(|path| body.pointer(path))
        .collect::<Vec<_>>();
    if ids.is_empty()
        || ids.iter().any(|id| id.as_str() != Some(ack.task_id()))
        || !matches!(response["kind"].as_str(), Some("result" | "error"))
    {
        return Err(Error::Forbidden);
    }
    if response["kind"] == "error"
        && (!body["error"]["code"].is_i64() || !body["error"]["message"].is_string())
    {
        return Err(Error::Forbidden);
    }
    if ack.status == cowork_mcp::TaskStatus::Cancelled {
        Ok("cancelled")
    } else if ack.status == cowork_mcp::TaskStatus::Failed
        || body["isError"] == true
        || response["kind"] == "error"
    {
        Ok("failed")
    } else {
        Ok("completed")
    }
}
fn state(ack: &TaskAcknowledgement) -> String {
    serde_json::to_value(&ack.status)
        .expect("task status")
        .as_str()
        .expect("task status")
        .into()
}
pub(crate) fn task_row(r: &AnyRow) -> Result<McpTaskRecord> {
    let revision: i64 = r.try_get("revision").map_err(storage_error)?;
    let record = McpTaskRecord {
        view: McpAsyncTask {
            id: r.try_get("id").map_err(storage_error)?,
            run_id: r.try_get("run_id").map_err(storage_error)?,
            tool_call_id: r.try_get("tool_call_id").map_err(storage_error)?,
            server_id: r.try_get("server_id").map_err(storage_error)?,
            revision,
            state: r.try_get("state").map_err(storage_error)?,
            protocol: r.try_get("protocol").map_err(storage_error)?,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            poll_interval_ms: u64::try_from(
                r.try_get::<i64, _>("poll_interval_ms")
                    .map_err(storage_error)?,
            )
            .map_err(|_| Error::Forbidden)?,
            operation_pending: r
                .try_get::<Option<String>, _>("pending_operation_id")
                .map_err(storage_error)?
                .is_some(),
            effects_unknown: true,
            result_sha256: r.try_get("final_result_sha256").map_err(storage_error)?,
        },
        owner_kind: r.try_get("owner_kind").map_err(storage_error)?,
        owner_id: r.try_get("owner_id").map_err(storage_error)?,
        project_id: r.try_get("project_id").map_err(storage_error)?,
        server_revision: r.try_get("server_revision").map_err(storage_error)?,
        binding_sha256: r.try_get("binding_sha256").map_err(storage_error)?,
        spec_sha256: r.try_get("spec_sha256").map_err(storage_error)?,
        connection_id: r.try_get("connection_id").map_err(storage_error)?,
        connection_epoch: u64::try_from(
            r.try_get::<i64, _>("connection_epoch")
                .map_err(storage_error)?,
        )
        .map_err(|_| Error::Forbidden)?,
        original_rpc_id: r.try_get("original_rpc_id").map_err(storage_error)?,
        original_request_sha256: r
            .try_get("original_request_sha256")
            .map_err(storage_error)?,
        transport_request_sha256: r
            .try_get("transport_request_sha256")
            .map_err(storage_error)?,
        task_id_sha256: r.try_get("task_id_sha256").map_err(storage_error)?,
        scope: parse(
            &r.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )?,
        sealed_ack: parse(
            &r.try_get::<String, _>("sealed_ack")
                .map_err(storage_error)?,
        )?,
        sealed_checkpoint: r
            .try_get::<Option<String>, _>("sealed_checkpoint")
            .map_err(storage_error)?
            .map(|v| parse(&v))
            .transpose()?,
        pending_operation_id: r.try_get("pending_operation_id").map_err(storage_error)?,
    };
    if record.view.protocol != MCP_PROTOCOL_VERSION
        || record.view.revision <= 0
        || record.server_revision <= 0
        || record.scope.owner_kind != record.owner_kind
        || record.scope.owner_id != record.owner_id
        || record.scope.project_id != record.project_id
        || record.scope.server_id != record.view.server_id
        || record.scope.record_id != record.view.id
        || record.scope.revision != revision
        || record.scope.configuration_sha256 != record.binding_sha256
        || record.scope.purpose != "task_ack"
    {
        return Err(Error::Forbidden);
    }
    record.scope.validate().map_err(|_| Error::Forbidden)?;
    Ok(record)
}
pub(crate) async fn lock_task(tx: &mut Transaction<'_, Any>, id: &str) -> Result<McpTaskRecord> {
    if sqlx::query("UPDATE mcp_async_tasks SET revision=revision WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    task_row(
        &sqlx::query("SELECT * FROM mcp_async_tasks WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )
}
impl McpTaskRecord {
    pub fn open(&self, keys: &dyn KeyStore) -> Result<McpTaskSecret> {
        let bytes = open_mcp_blob(keys, &self.scope, &self.sealed_ack)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let secret: McpTaskSecret = serde_json::from_slice(&bytes).map_err(storage_error)?;
        secret.original.validate().map_err(|_| Error::Forbidden)?;
        secret.current.validate().map_err(|_| Error::Forbidden)?;
        if secret.original_rpc_response["task"] != *secret.original.original()
            || canonical_hash(&secret.original.task_id()) != self.task_id_sha256
            || secret.current.task_id() != secret.original.task_id()
            || secret.current.created_at_ms != secret.original.created_at_ms
            || secret.current.ttl_ms != secret.original.ttl_ms
            || secret.current.last_updated_at_ms < secret.original.last_updated_at_ms
            || self.view.expires_at != secret.original.expires_at_ms()
        {
            return Err(Error::Forbidden);
        }
        Ok(secret)
    }
}
async fn native_authority(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    run: &str,
    lease: &str,
    call: &str,
    key: KeyId,
) -> Result<(StoredRun, StoredMcpAttempt, String)> {
    if !store.is_local_sqlite() {
        return Err(Error::Forbidden);
    }
    let keys = crate::native_secrets::lock_state(tx).await?;
    if keys.active.as_ref().is_none_or(|p| p.key != key) || keys.pending_operation.is_some() {
        return Err(Error::NativeKeyUnavailable);
    }
    crate::native_key_retirement::allow_cipher(tx, &key.cipher_id().map_err(|_| Error::Forbidden)?)
        .await?;
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
    let stored: StoredRun = parse(
        &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
            .bind(run)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )?;
    stored.assert_lease(lease, now())?;
    if stored.record.id != run
        || stored.record.state != RunState::Running
        || cowork_domain::time_budget_exhausted(&stored.record, now())
    {
        return Err(Error::Conflict);
    }
    let r = sqlx::query("SELECT payload FROM mcp_attempts WHERE run_id=$1 AND tool_call_id=$2")
        .bind(run)
        .bind(call)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let attempt: StoredMcpAttempt =
        parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    attempt.validate_binding(&stored.record)?;
    if attempt.record.state != McpAttemptState::DispatchCommitted
        || attempt.lease_sha256 != hash(lease.as_bytes())
        || stored.checkpoint.in_flight.as_ref() != Some(&attempt.record.tool)
        || attempt.record.transport_task_ttl_ms.is_none()
        || attempt.record.transport_protocol.is_some()
    {
        return Err(Error::Forbidden);
    }
    crate::mcp_servers::check_binding(tx, &stored.record, &attempt.record.binding).await?;
    let authority = canonical_hash(&json!([
        keys.instance,
        keys.revision,
        stored.record.spec,
        attempt.record,
        hash(lease.as_bytes())
    ]));
    Ok((stored, attempt, authority))
}
fn check_original(
    task: &McpTaskRecord,
    stored: &StoredRun,
    attempt: &StoredMcpAttempt,
    key: KeyId,
) -> Result<()> {
    if task.owner_kind != "native"
        || task.owner_id != key.instance.to_string()
        || task.project_id != stored.record.spec.project_id
        || task.view.run_id != stored.record.id
        || task.view.tool_call_id != attempt.record.tool.id
        || task.view.server_id != attempt.record.binding.server_id
        || task.binding_sha256 != canonical_hash(&attempt.record.binding)
        || task.spec_sha256 != canonical_hash(&stored.record.spec)
        || task.connection_id != attempt.record.connection_id
        || task.original_rpc_id != attempt.record.rpc_request_id
        || task.original_request_sha256 != attempt.record.request_sha256
        || task.transport_request_sha256 != attempt.record.transport_request_sha256
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn finish_mcp_task_evidence_unknown(
        &self,
        permit: &McpTaskOperationPermit,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, &permit.task.view.id).await?;
        check_operation(&mut tx, &task, permit).await?;
        let row=sqlx::query("SELECT response_sha256 FROM mcp_async_task_operations WHERE id=$1 AND state='received' AND sealed_response IS NOT NULL").bind(&permit.id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let receipt = json!({"operation_id":permit.id,"task_record_id":task.view.id,"state":"unknown","effects_unknown":true,"response_sha256":row.try_get::<String,_>("response_sha256").map_err(storage_error)?});
        sqlx::query("UPDATE mcp_async_task_operations SET state='committed',receipt=$1 WHERE id=$2 AND state='received'").bind(json_string(&receipt)?).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_async_tasks SET state='unknown',updated_at=$1 WHERE id=$2 AND pending_operation_id=$3").bind(now()).bind(&task.view.id).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_task_interactions SET state='unknown',revision=revision+1,updated_at=$1 WHERE operation_id=$2 AND state NOT IN('responded','unknown')").bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_task_sampling_claims SET state='unknown',updated_at=$1 WHERE interaction_id IN(SELECT id FROM mcp_task_interactions WHERE operation_id=$2) AND state='admitted'").bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn request_native_mcp_task_cancel(
        &self,
        run: &str,
        id: &str,
        input: &McpAsyncTaskRequest,
    ) -> Result<Value> {
        valid_text(&input.idempotency_key, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        if !self.is_local_sqlite() || state.active.is_none() || state.pending_operation.is_some() {
            return Err(Error::Forbidden);
        }
        let task = lock_task(&mut tx, id).await?;
        if task.owner_kind != "native"
            || task.owner_id != state.instance.to_string()
            || task.view.run_id != run
        {
            return Err(Error::Forbidden);
        }
        if let Some(row)=sqlx::query("SELECT expected_revision,receipt FROM mcp_task_control_requests WHERE task_record_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<i64,_>("expected_revision").map_err(storage_error)?!=input.expected_revision{return Err(Error::Conflict);}return parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?);}
        let stored: StoredRun = parse(
            &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                .bind(run)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?,
        )?;
        if stored.record.state != RunState::Running
            || stored.record.spec.project_id != task.project_id
            || task.view.revision != input.expected_revision
            || task.pending_operation_id.is_some()
            || !matches!(task.view.state.as_str(), "working" | "input_required")
        {
            return Err(Error::Conflict);
        }
        let receipt = json!({"task_record_id":id,"expected_revision":input.expected_revision,"request_key":input.idempotency_key,"state":"requested","effects_unknown":true});
        sqlx::query("INSERT INTO mcp_task_control_requests(task_record_id,request_key,expected_revision,operation,state,original_actor,receipt,created_at) VALUES($1,$2,$3,'cancel','requested',$4,$5,$6)").bind(id).bind(&input.idempotency_key).bind(input.expected_revision).bind(state.instance.to_string()).bind(json_string(&receipt)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn native_mcp_task_cancel_requested(&self, task: &McpTaskRecord) -> Result<bool> {
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_task_control_requests WHERE task_record_id=$1 AND expected_revision=$2 AND state='requested' AND original_actor=$3").bind(&task.view.id).bind(task.view.revision).bind(&task.owner_id).fetch_one(&self.pool).await.map_err(storage_error)?;
        Ok(count == 1)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "capture_native_mcp_task: Keep original MCP call/task/session, live lease and native key custody bindings explicit"
    )]
    pub async fn capture_native_mcp_task(
        &self,
        run: &str,
        lease: &str,
        call: &str,
        ack: &TaskAcknowledgement,
        response: &Value,
        session: &Session,
        cipher: Option<&SessionCipher>,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        ack.validate().map_err(|_| Error::Forbidden)?;
        if !session.has_pending_task() || session.protocol_version() != MCP_PROTOCOL_VERSION {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (stored, attempt, _) = native_authority(self, &mut tx, run, lease, call, key).await?;
        if session.connection_id() != attempt.record.connection_id {
            return Err(Error::Forbidden);
        }
        if let Some(r)=sqlx::query("SELECT * FROM mcp_async_tasks WHERE owner_kind='native' AND run_id=$1 AND tool_call_id=$2").bind(run).bind(call).fetch_optional(&mut *tx).await.map_err(storage_error)? {let task=task_row(&r)?;check_original(&task,&stored,&attempt,key)?;if task.open(keys)?.original!=*ack{return Err(Error::Conflict);}return Ok(task);}
        let id = Uuid::new_v4().to_string();
        let epoch = Uuid::new_v4();
        let binding = canonical_hash(&attempt.record.binding);
        let scope = McpSecretScope {
            owner_kind: "native".into(),
            owner_id: key.instance.to_string(),
            project_id: stored.record.spec.project_id.clone(),
            server_id: attempt.record.binding.server_id.clone(),
            record_id: id.clone(),
            revision: 1,
            epoch,
            configuration_sha256: binding.clone(),
            purpose: "task_ack".into(),
        };
        if response["task"] != *ack.original() {
            return Err(Error::Forbidden);
        }
        let secret = McpTaskSecret {
            original: ack.clone(),
            current: ack.clone(),
            original_rpc_response: response.clone(),
        };
        let sealed = seal_mcp_blob(
            keys,
            key,
            &scope,
            &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let checkpoint = if let Some(cipher) = cipher {
            let row=sqlx::query("SELECT id,server_id,revision,payload FROM mcp_connections WHERE run_id=$1 AND server_id=$2").bind(run).bind(&attempt.record.binding.server_id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::McpSessionUnavailable)?;
            let connection = crate::mcp_connections::checked(&row, &stored.record)?;
            let original_scope = connection.scope(&stored.record, connection.next_revision()?)?;
            let sealed_session = cipher
                .seal(session, &original_scope)
                .map_err(|_| Error::McpSessionUnavailable)?;
            let mut s = scope.clone();
            s.purpose = "task_checkpoint".into();
            Some(
                seal_mcp_blob(
                    keys,
                    key,
                    &s,
                    &Zeroizing::new(
                        serde_json::to_vec(
                            &json!({"scope":original_scope,"sealed":sealed_session}),
                        )
                        .map_err(storage_error)?,
                    ),
                )
                .map_err(|_| Error::NativeKeyUnavailable)?,
            )
        } else {
            None
        };
        sqlx::query("INSERT INTO mcp_async_tasks(id,owner_kind,owner_id,project_id,run_id,tool_call_id,server_id,server_revision,binding_sha256,spec_sha256,connection_id,connection_epoch,protocol,original_rpc_id,original_request_sha256,transport_request_sha256,task_id_sha256,revision,state,expires_at,poll_interval_ms,secret_scope,sealed_ack,sealed_checkpoint,checkpoint_key_id,created_at,updated_at) VALUES($1,'native',$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,1,$17,$18,$19,$20,$21,$22,$23,$24,$24)")
            .bind(&id).bind(key.instance.to_string()).bind(&stored.record.spec.project_id).bind(run).bind(call).bind(&attempt.record.binding.server_id).bind(attempt.record.binding.server_revision).bind(binding).bind(canonical_hash(&stored.record.spec)).bind(session.connection_id()).bind(i64::try_from(session.epoch()).map_err(|_|Error::Forbidden)?).bind(MCP_PROTOCOL_VERSION).bind(&attempt.record.rpc_request_id).bind(&attempt.record.request_sha256).bind(&attempt.record.transport_request_sha256).bind(canonical_hash(&ack.task_id())).bind(state(ack)).bind(ack.expires_at_ms()).bind(ack.poll_interval_ms as i64).bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(checkpoint.as_ref().map(json_string).transpose()?).bind(checkpoint.as_ref().map(|_|cipher.map(|c|c.key_id()).ok_or(Error::McpSessionUnavailable)).transpose()?.map(|id|cowork_secrets::KeyId::from_cipher_id(id).map_err(|_|Error::Forbidden)).transpose()?.as_ref().map(json_string).transpose()?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        let record = task_row(
            &sqlx::query("SELECT * FROM mcp_async_tasks WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        )?;
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn begin_native_mcp_task_operation(
        &self,
        run: &str,
        lease: &str,
        task: &McpTaskRecord,
        prepared: &PreparedTaskOperation,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskOperationPermit> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (stored, attempt, authority) =
            native_authority(self, &mut tx, run, lease, &task.view.tool_call_id, key).await?;
        let current = lock_task(&mut tx, &task.view.id).await?;
        check_original(&current, &stored, &attempt, key)?;
        if current.view.revision != task.view.revision
            || current.pending_operation_id.is_some()
            || current.view.expires_at.is_some_and(|v| v <= now())
            || current.view.state == "unknown"
            || current.view.state == "expired"
        {
            return Err(Error::UncertainEffect);
        }
        let original = current.open(keys)?;
        if prepared.connection_id() != current.connection_id
            || prepared.task_id() != original.original.task_id()
            || prepared.original_rpc_id() != current.original_rpc_id
            || prepared.original_request_sha256() != current.transport_request_sha256
        {
            return Err(Error::Forbidden);
        }
        if prepared.operation()==TaskOperation::Cancel&&sqlx::query("UPDATE mcp_task_control_requests SET state='admitted' WHERE task_record_id=$1 AND expected_revision=$2 AND original_actor=$3 AND state='requested'").bind(&task.view.id).bind(task.view.revision).bind(&task.owner_id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Forbidden);}
        let id = Uuid::new_v4().to_string();
        let mut scope = current.scope.clone();
        scope.record_id = id.clone();
        scope.purpose = "task_response".into();
        sqlx::query("INSERT INTO mcp_async_task_operations(id,task_record_id,expected_revision,operation,rpc_request_id,request_sha256,authority_sha256,state,secret_scope,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'admitted',$8,$9,$9)").bind(&id).bind(&task.view.id).bind(task.view.revision).bind(match prepared.operation(){TaskOperation::Get=>"get",TaskOperation::Result=>"result",TaskOperation::Cancel=>"cancel"}).bind(prepared.rpc_request_id()).bind(prepared.request_sha256()).bind(&authority).bind(json_string(&scope)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_async_tasks SET pending_operation_id=$1 WHERE id=$2 AND revision=$3 AND pending_operation_id IS NULL").bind(&id).bind(&task.view.id).bind(task.view.revision).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(McpTaskOperationPermit {
            id,
            task: current,
            operation: prepared.operation(),
            request_sha256: prepared.request_sha256().into(),
            rpc_request_id: prepared.rpc_request_id().into(),
            authority_sha256: authority,
            scope,
        })
    }
    pub async fn validate_native_mcp_task_operation(
        &self,
        run: &str,
        lease: &str,
        permit: &McpTaskOperationPermit,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (stored, attempt, authority) = native_authority(
            self,
            &mut tx,
            run,
            lease,
            &permit.task.view.tool_call_id,
            key,
        )
        .await?;
        let task = lock_task(&mut tx, &permit.task.view.id).await?;
        check_original(&task, &stored, &attempt, key)?;
        check_operation(&mut tx, &task, permit).await?;
        if authority != permit.authority_sha256 || task.view.expires_at.is_some_and(|v| v <= now())
        {
            return Err(Error::Forbidden);
        }
        let state: String =
            sqlx::query_scalar("SELECT state FROM mcp_async_task_operations WHERE id=$1")
                .bind(&permit.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if state != "admitted" {
            return Err(Error::UncertainEffect);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn capture_mcp_task_operation(
        &self,
        permit: &McpTaskOperationPermit,
        response: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(response).map_err(storage_error)?);
        let sealed = seal_mcp_blob(keys, key, &permit.scope, &plain)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::mcp_oauth::evidence_cipher(&mut tx, &permit.scope.owner_kind, key).await?;
        let r = sqlx::query("SELECT * FROM mcp_async_task_operations WHERE id=$1")
            .bind(&permit.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        if r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != permit.request_sha256
            || parse::<McpSecretScope>(
                &r.try_get::<String, _>("secret_scope")
                    .map_err(storage_error)?,
            )? != permit.scope
        {
            return Err(Error::Forbidden);
        }
        if let Some(old) = r
            .try_get::<Option<String>, _>("sealed_response")
            .map_err(storage_error)?
        {
            if open_mcp_blob(keys, &permit.scope, &parse(&old)?)
                .map_err(|_| Error::NativeKeyUnavailable)?
                .as_slice()
                != plain.as_slice()
            {
                return Err(Error::Conflict);
            }
            return Ok(());
        }
        if sqlx::query("UPDATE mcp_async_task_operations SET state='received',sealed_response=$1,response_sha256=$2,updated_at=$3 WHERE id=$4 AND state='admitted' AND sealed_response IS NULL").bind(json_string(&sealed)?).bind(canonical_hash(response)).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect);}
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn commit_native_mcp_task_operation(
        &self,
        run: &str,
        lease: &str,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (stored, attempt, authority) = native_authority(
            self,
            &mut tx,
            run,
            lease,
            &permit.task.view.tool_call_id,
            key,
        )
        .await?;
        let mut task = lock_task(&mut tx, &permit.task.view.id).await?;
        check_original(&task, &stored, &attempt, key)?;
        check_operation(&mut tx, &task, permit).await?;
        if authority != permit.authority_sha256 {
            return Err(Error::Forbidden);
        }
        let r = sqlx::query("SELECT * FROM mcp_async_task_operations WHERE id=$1")
            .bind(&permit.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if r.try_get::<String, _>("state").map_err(storage_error)? != "received" {
            return Err(Error::UncertainEffect);
        }
        let response: Value = serde_json::from_slice(
            &open_mcp_blob(
                keys,
                &permit.scope,
                &parse(
                    &r.try_get::<String, _>("sealed_response")
                        .map_err(storage_error)?,
                )?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if canonical_hash(&response)
            != r.try_get::<String, _>("response_sha256")
                .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let mut secret = task.open(keys)?;
        task.view.revision = task.view.revision.checked_add(1).ok_or(Error::Conflict)?;
        if permit.operation == TaskOperation::Result {
            task.view.state = result_state(&secret.current, &response)?.into();
            task.view.result_sha256 = Some(canonical_hash(&response["response"]));
            sqlx::query("UPDATE mcp_async_tasks SET final_result_scope=$1,sealed_final_result=$2,final_result_sha256=$3 WHERE id=$4 AND sealed_final_result IS NULL").bind(json_string(&permit.scope)?).bind(r.try_get::<String,_>("sealed_response").map_err(storage_error)?).bind(task.view.result_sha256.clone()).bind(&task.view.id).execute(&mut *tx).await.map_err(storage_error)?;
        } else {
            if response["kind"] != "status" {
                return Err(Error::Forbidden);
            }
            let next: TaskAcknowledgement =
                serde_json::from_value(response["ack"].clone()).map_err(storage_error)?;
            next.validate().map_err(|_| Error::Forbidden)?;
            if next.task_id() != secret.original.task_id()
                || next.created_at_ms != secret.original.created_at_ms
                || next.ttl_ms != secret.original.ttl_ms
                || next.last_updated_at_ms < secret.current.last_updated_at_ms
            {
                return Err(Error::Forbidden);
            }
            task.view.state = state(&next);
            task.view.poll_interval_ms = next.poll_interval_ms;
            secret.current = next;
        }
        task.scope.revision = task.view.revision;
        let blob = seal_mcp_blob(
            keys,
            key,
            &task.scope,
            &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        // The original checkpoint retains its original AAD revision. Rewrap may
        // change its key only, never relabel that original transport authority.
        sqlx::query("UPDATE mcp_async_tasks SET revision=$1,state=$2,poll_interval_ms=$3,secret_scope=$4,sealed_ack=$5,pending_operation_id=NULL,updated_at=$6 WHERE id=$7 AND revision=$8 AND pending_operation_id=$9").bind(task.view.revision).bind(&task.view.state).bind(task.view.poll_interval_ms as i64).bind(json_string(&task.scope)?).bind(json_string(&blob)?).bind(now()).bind(&task.view.id).bind(permit.task.view.revision).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        let receipt = json!({"operation_id":permit.id,"task_record_id":task.view.id,"task_revision":task.view.revision,"state":task.view.state,"response_sha256":canonical_hash(&response),"effects_unknown":true});
        sqlx::query("UPDATE mcp_async_task_operations SET state='committed',receipt=$1,updated_at=$2 WHERE id=$3 AND state='received'").bind(json_string(&receipt)?).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_task_control_requests SET state=$1 WHERE task_record_id=$2 AND expected_revision=$3 AND state IN('requested','admitted')").bind(if permit.operation==TaskOperation::Cancel{"completed"}else{"stale"}).bind(&task.view.id).bind(permit.task.view.revision).execute(&mut *tx).await.map_err(storage_error)?;
        let result = task_row(
            &sqlx::query("SELECT * FROM mcp_async_tasks WHERE id=$1")
                .bind(&task.view.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        )?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn unknown_mcp_task_operation(&self, permit: &McpTaskOperationPermit) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let receipt = json!({"operation_id":permit.id,"task_record_id":permit.task.view.id,"state":"unknown","effects_unknown":true});
        if sqlx::query("UPDATE mcp_async_task_operations SET state='unknown',receipt=$1,updated_at=$2 WHERE id=$3 AND state='admitted' AND sealed_response IS NULL").bind(json_string(&receipt)?).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        sqlx::query("UPDATE mcp_async_tasks SET state='unknown',updated_at=$1 WHERE id=$2 AND pending_operation_id=$3").bind(now()).bind(&permit.task.view.id).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_task_interactions SET state='unknown',revision=revision+1,updated_at=$1 WHERE operation_id=$2 AND state NOT IN('responded','unknown')").bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_task_sampling_claims SET state='unknown',updated_at=$1 WHERE interaction_id IN(SELECT id FROM mcp_task_interactions WHERE operation_id=$2) AND state='admitted'").bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn native_mcp_tasks(&self, run: &str) -> Result<Vec<McpAsyncTask>> {
        let stored = self.stored_run(run).await?;
        let state = self
            .native_secret_state()
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let rows=sqlx::query("SELECT * FROM mcp_async_tasks WHERE owner_kind='native' AND owner_id=$1 AND project_id=$2 AND run_id=$3 ORDER BY created_at LIMIT 129").bind(state.instance.to_string()).bind(&stored.record.spec.project_id).bind(run).fetch_all(&self.pool).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        rows.iter().map(|r| Ok(task_row(r)?.view)).collect()
    }
}
pub(crate) async fn check_operation(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    permit: &McpTaskOperationPermit,
) -> Result<()> {
    if task.view.revision != permit.task.view.revision
        || task.pending_operation_id.as_deref() != Some(&permit.id)
        || task.scope.epoch != permit.task.scope.epoch
    {
        return Err(Error::Forbidden);
    }
    let r=sqlx::query("SELECT task_record_id,expected_revision,rpc_request_id,request_sha256,authority_sha256,secret_scope FROM mcp_async_task_operations WHERE id=$1").bind(&permit.id).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if r.try_get::<String, _>("task_record_id")
        .map_err(storage_error)?
        != task.view.id
        || r.try_get::<i64, _>("expected_revision")
            .map_err(storage_error)?
            != task.view.revision
        || r.try_get::<String, _>("rpc_request_id")
            .map_err(storage_error)?
            != permit.rpc_request_id
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != permit.request_sha256
        || r.try_get::<String, _>("authority_sha256")
            .map_err(storage_error)?
            != permit.authority_sha256
        || parse::<McpSecretScope>(
            &r.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )? != permit.scope
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
