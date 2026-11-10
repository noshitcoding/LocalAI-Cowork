//! Journal operations used only inside an already-authorized Team/private-node
//! writer transaction. Facts come from original encrypted product intents.
use super::*;
use crate::mcp_tasks::{
    McpTaskOperationPermit, McpTaskRecord, McpTaskSecret, check_operation, lock_task,
};
use cowork_mcp::{
    CallResult, CheckpointScope, PreparedTaskOperation, Session, SessionCipher,
    TaskAcknowledgement, TaskOperation,
};
use cowork_secrets::{
    KeyId, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, seal_mcp_blob,
};
use sqlx::{Any, Transaction};
use zeroize::Zeroizing;
pub(crate) struct Facts {
    pub owner_kind: String,
    pub owner_id: String,
    pub project_id: String,
    pub run_id: String,
    pub call_id: String,
    pub server_id: String,
    pub server_revision: i64,
    pub binding_sha256: String,
    pub spec_sha256: String,
    pub connection_id: String,
    pub connection_epoch: u64,
    pub original_rpc_id: String,
    pub original_request_sha256: String,
    pub transport_request_sha256: String,
    pub authority_sha256: String,
}
pub(crate) fn check(f: &Facts, t: &McpTaskRecord) -> Result<()> {
    if t.owner_kind != f.owner_kind
        || t.owner_id != f.owner_id
        || t.project_id != f.project_id
        || t.view.run_id != f.run_id
        || t.view.tool_call_id != f.call_id
        || t.view.server_id != f.server_id
        || t.server_revision != f.server_revision
        || t.binding_sha256 != f.binding_sha256
        || t.spec_sha256 != f.spec_sha256
        || t.connection_id != f.connection_id
        || t.connection_epoch != f.connection_epoch
        || t.original_rpc_id != f.original_rpc_id
        || t.original_request_sha256 != f.original_request_sha256
        || t.transport_request_sha256 != f.transport_request_sha256
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn capture(
    tx: &mut Transaction<'_, Any>,
    f: &Facts,
    result: &CallResult,
    session: &Session,
    cipher: Option<(&SessionCipher, CheckpointScope)>,
    keys: &dyn KeyStore,
    key: KeyId,
) -> Result<McpTaskRecord> {
    let ack = result.task.as_ref().ok_or(Error::Forbidden)?;
    ack.validate().map_err(|_| Error::Forbidden)?;
    if !matches!(
        f.owner_kind.as_str(),
        "native" | "personal" | "team" | "offline"
    ) || session.protocol_version() != MCP_PROTOCOL_VERSION
        || session.connection_id() != f.connection_id
        || session.epoch() != f.connection_epoch
        || !session.has_pending_task()
        || result.response["task"] != *ack.original()
    {
        return Err(Error::Forbidden);
    }
    crate::mcp_oauth::evidence_cipher(tx, &f.owner_kind, key).await?;
    if let Some(row) = sqlx::query(
        "SELECT id FROM mcp_async_tasks WHERE owner_kind=$1 AND run_id=$2 AND tool_call_id=$3",
    )
    .bind(&f.owner_kind)
    .bind(&f.run_id)
    .bind(&f.call_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    {
        let t = lock_task(tx, &row.try_get::<String, _>("id").map_err(storage_error)?).await?;
        check(f, &t)?;
        if t.open(keys)?.original_rpc_response != result.response {
            return Err(Error::Conflict);
        }
        return Ok(t);
    }
    let id = Uuid::new_v4().to_string();
    let scope = McpSecretScope {
        owner_kind: f.owner_kind.clone(),
        owner_id: f.owner_id.clone(),
        project_id: f.project_id.clone(),
        server_id: f.server_id.clone(),
        record_id: id.clone(),
        revision: 1,
        epoch: Uuid::new_v4(),
        configuration_sha256: f.binding_sha256.clone(),
        purpose: "task_ack".into(),
    };
    let secret = McpTaskSecret {
        original: ack.clone(),
        current: ack.clone(),
        original_rpc_response: result.response.clone(),
    };
    let sealed = seal_mcp_blob(
        keys,
        key,
        &scope,
        &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    let (checkpoint, inner_key) = if let Some((cipher, original_scope)) = cipher {
        if original_scope.project_id != f.project_id
            || original_scope.run_id != f.run_id
            || original_scope.server_id != f.server_id
            || original_scope.server_revision != f.server_revision
        {
            return Err(Error::Forbidden);
        }
        let inner = cipher
            .seal(session, &original_scope)
            .map_err(|_| Error::McpSessionUnavailable)?;
        let inner_key = KeyId::from_cipher_id(inner.key_id()).map_err(|_| Error::Forbidden)?;
        let mut s = scope.clone();
        s.purpose = "task_checkpoint".into();
        let blob = seal_mcp_blob(
            keys,
            key,
            &s,
            &Zeroizing::new(
                serde_json::to_vec(&json!({"scope":original_scope,"sealed":inner}))
                    .map_err(storage_error)?,
            ),
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        (Some(blob), Some(inner_key))
    } else {
        (None, None)
    };
    let status = serde_json::to_value(&ack.status)
        .map_err(storage_error)?
        .as_str()
        .ok_or(Error::Forbidden)?
        .to_owned();
    sqlx::query("INSERT INTO mcp_async_tasks(id,owner_kind,owner_id,project_id,run_id,tool_call_id,server_id,server_revision,binding_sha256,spec_sha256,connection_id,connection_epoch,protocol,original_rpc_id,original_request_sha256,transport_request_sha256,task_id_sha256,revision,state,expires_at,poll_interval_ms,secret_scope,sealed_ack,sealed_checkpoint,checkpoint_key_id,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,1,$18,$19,$20,$21,$22,$23,$24,$25,$25)")
  .bind(&id).bind(&f.owner_kind).bind(&f.owner_id).bind(&f.project_id).bind(&f.run_id).bind(&f.call_id).bind(&f.server_id).bind(f.server_revision).bind(&f.binding_sha256).bind(&f.spec_sha256).bind(&f.connection_id).bind(i64::try_from(f.connection_epoch).map_err(|_|Error::Forbidden)?).bind(MCP_PROTOCOL_VERSION).bind(&f.original_rpc_id).bind(&f.original_request_sha256).bind(&f.transport_request_sha256).bind(canonical_hash(&ack.task_id())).bind(status).bind(ack.expires_at_ms()).bind(ack.poll_interval_ms as i64).bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(checkpoint.as_ref().map(json_string).transpose()?).bind(inner_key.as_ref().map(json_string).transpose()?).bind(now()).execute(&mut **tx).await.map_err(storage_error)?;
    lock_task(tx, &id).await
}
pub(crate) async fn begin(
    tx: &mut Transaction<'_, Any>,
    f: &Facts,
    task: &McpTaskRecord,
    prepared: &PreparedTaskOperation,
    keys: &dyn KeyStore,
    key: KeyId,
) -> Result<McpTaskOperationPermit> {
    crate::mcp_oauth::evidence_cipher(tx, &f.owner_kind, key).await?;
    let current = lock_task(tx, &task.view.id).await?;
    check(f, &current)?;
    let original = current.open(keys)?;
    if current.view.revision != task.view.revision
        || current.pending_operation_id.is_some()
        || current.view.expires_at.is_some_and(|v| v <= now())
        || matches!(current.view.state.as_str(), "unknown" | "expired")
        || matches!(current.view.state.as_str(), "cancelled" | "failed")
            && prepared.operation() != TaskOperation::Result
        || prepared.connection_id() != f.connection_id
        || prepared.task_id() != original.original.task_id()
        || prepared.original_rpc_id() != f.original_rpc_id
        || prepared.original_request_sha256() != f.transport_request_sha256
    {
        return Err(Error::UncertainEffect);
    }
    let id = Uuid::new_v4().to_string();
    let mut scope = current.scope.clone();
    scope.record_id = id.clone();
    scope.purpose = "task_response".into();
    let operation = match prepared.operation() {
        TaskOperation::Get => "get",
        TaskOperation::Result => "result",
        TaskOperation::Cancel => "cancel",
    };
    if prepared.operation() == TaskOperation::Cancel {
        let row=sqlx::query("SELECT original_actor FROM mcp_task_control_requests WHERE task_record_id=$1 AND expected_revision=$2 AND state='requested'").bind(&task.view.id).bind(task.view.revision).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        crate::mcp_interactions::control_current(
            tx,
            &current,
            &row.try_get::<String, _>("original_actor")
                .map_err(storage_error)?,
        )
        .await?;
        if sqlx::query("UPDATE mcp_task_control_requests SET state='admitted' WHERE task_record_id=$1 AND expected_revision=$2 AND state='requested'").bind(&task.view.id).bind(task.view.revision).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    }
    sqlx::query("INSERT INTO mcp_async_task_operations(id,task_record_id,expected_revision,operation,rpc_request_id,request_sha256,authority_sha256,state,secret_scope,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'admitted',$8,$9,$9)").bind(&id).bind(&task.view.id).bind(task.view.revision).bind(operation).bind(prepared.rpc_request_id()).bind(prepared.request_sha256()).bind(&f.authority_sha256).bind(json_string(&scope)?).bind(now()).execute(&mut **tx).await.map_err(storage_error)?;
    if sqlx::query("UPDATE mcp_async_tasks SET pending_operation_id=$1 WHERE id=$2 AND revision=$3 AND pending_operation_id IS NULL").bind(&id).bind(&task.view.id).bind(task.view.revision).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    Ok(McpTaskOperationPermit {
        id,
        task: current,
        operation: prepared.operation(),
        request_sha256: prepared.request_sha256().into(),
        rpc_request_id: prepared.rpc_request_id().into(),
        authority_sha256: f.authority_sha256.clone(),
        scope,
    })
}
pub(crate) async fn validate(
    tx: &mut Transaction<'_, Any>,
    f: &Facts,
    permit: &McpTaskOperationPermit,
    key: KeyId,
) -> Result<()> {
    crate::mcp_oauth::evidence_cipher(tx, &f.owner_kind, key).await?;
    let task = lock_task(tx, &permit.task.view.id).await?;
    check(f, &task)?;
    check_operation(tx, &task, permit).await?;
    if f.authority_sha256 != permit.authority_sha256
        || task.view.expires_at.is_some_and(|v| v <= now())
    {
        return Err(Error::Forbidden);
    }
    if permit.operation == TaskOperation::Cancel {
        let raw:String=sqlx::query_scalar("SELECT original_actor FROM mcp_task_control_requests WHERE task_record_id=$1 AND expected_revision=$2 AND state='admitted'").bind(&task.view.id).bind(task.view.revision).fetch_one(&mut **tx).await.map_err(storage_error)?;
        crate::mcp_interactions::control_current(tx, &task, &raw).await?;
    }
    let state: String =
        sqlx::query_scalar("SELECT state FROM mcp_async_task_operations WHERE id=$1")
            .bind(&permit.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if state != "admitted" {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}
pub(crate) async fn commit(
    tx: &mut Transaction<'_, Any>,
    f: &Facts,
    permit: &McpTaskOperationPermit,
    keys: &dyn KeyStore,
    key: KeyId,
) -> Result<McpTaskRecord> {
    crate::mcp_oauth::evidence_cipher(tx, &f.owner_kind, key).await?;
    let mut task = lock_task(tx, &permit.task.view.id).await?;
    check(f, &task)?;
    check_operation(tx, &task, permit).await?;
    if f.authority_sha256 != permit.authority_sha256 {
        return Err(Error::Forbidden);
    }
    if permit.operation == TaskOperation::Cancel {
        let raw:String=sqlx::query_scalar("SELECT original_actor FROM mcp_task_control_requests WHERE task_record_id=$1 AND expected_revision=$2 AND state='admitted'").bind(&task.view.id).bind(task.view.revision).fetch_one(&mut **tx).await.map_err(storage_error)?;
        crate::mcp_interactions::control_current(tx, &task, &raw).await?;
    }
    let row = sqlx::query(
        "SELECT state,sealed_response,response_sha256 FROM mcp_async_task_operations WHERE id=$1",
    )
    .bind(&permit.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if row.try_get::<String, _>("state").map_err(storage_error)? != "received" {
        return Err(Error::UncertainEffect);
    }
    let blob: SealedMcpBlob = parse(
        &row.try_get::<String, _>("sealed_response")
            .map_err(storage_error)?,
    )?;
    let response: Value = serde_json::from_slice(
        &open_mcp_blob(keys, &permit.scope, &blob).map_err(|_| Error::NativeKeyUnavailable)?,
    )
    .map_err(storage_error)?;
    if canonical_hash(&response)
        != row
            .try_get::<String, _>("response_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let mut secret = task.open(keys)?;
    if permit.operation == TaskOperation::Result {
        task.view.state = crate::mcp_tasks::result_state(&secret.current, &response)?.into();
        let digest = canonical_hash(&response["response"]);
        sqlx::query("UPDATE mcp_async_tasks SET final_result_scope=$1,sealed_final_result=$2,final_result_sha256=$3 WHERE id=$4 AND sealed_final_result IS NULL").bind(json_string(&permit.scope)?).bind(json_string(&blob)?).bind(digest).bind(&task.view.id).execute(&mut **tx).await.map_err(storage_error)?;
    } else {
        if response["kind"] != "status" {
            return Err(Error::Forbidden);
        }
        let ack: TaskAcknowledgement =
            serde_json::from_value(response["ack"].clone()).map_err(storage_error)?;
        ack.validate().map_err(|_| Error::Forbidden)?;
        if ack.task_id() != secret.original.task_id()
            || ack.created_at_ms != secret.original.created_at_ms
            || ack.ttl_ms != secret.original.ttl_ms
            || ack.last_updated_at_ms < secret.current.last_updated_at_ms
            || secret.current.status.terminal() && secret.current.status != ack.status
            || permit.operation == TaskOperation::Cancel
                && ack.status != cowork_mcp::TaskStatus::Cancelled
        {
            return Err(Error::Forbidden);
        }
        task.view.state = serde_json::to_value(&ack.status)
            .map_err(storage_error)?
            .as_str()
            .ok_or(Error::Forbidden)?
            .into();
        task.view.poll_interval_ms = ack.poll_interval_ms;
        secret.current = ack;
    }
    task.view.revision = task.view.revision.checked_add(1).ok_or(Error::Conflict)?;
    task.scope.revision = task.view.revision;
    let sealed = seal_mcp_blob(
        keys,
        key,
        &task.scope,
        &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    if sqlx::query("UPDATE mcp_async_tasks SET revision=$1,state=$2,poll_interval_ms=$3,secret_scope=$4,sealed_ack=$5,pending_operation_id=NULL,updated_at=$6 WHERE id=$7 AND revision=$8 AND pending_operation_id=$9").bind(task.view.revision).bind(&task.view.state).bind(task.view.poll_interval_ms as i64).bind(json_string(&task.scope)?).bind(json_string(&sealed)?).bind(now()).bind(&task.view.id).bind(permit.task.view.revision).bind(&permit.id).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    let receipt = json!({"operation_id":permit.id,"task_record_id":task.view.id,"task_revision":task.view.revision,"state":task.view.state,"response_sha256":canonical_hash(&response),"effects_unknown":true});
    if sqlx::query("UPDATE mcp_async_task_operations SET state='committed',receipt=$1,updated_at=$2 WHERE id=$3 AND state='received'").bind(json_string(&receipt)?).bind(now()).bind(&permit.id).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    sqlx::query("UPDATE mcp_task_control_requests SET state=$1 WHERE task_record_id=$2 AND expected_revision=$3 AND state IN('requested','admitted')").bind(if permit.operation==TaskOperation::Cancel{"completed"}else{"stale"}).bind(&task.view.id).bind(permit.task.view.revision).execute(&mut **tx).await.map_err(storage_error)?;
    lock_task(tx, &task.view.id).await
}
