//! Encrypted original server requests, explicit human decisions and one response
//! send intent. The journal never manufactures an execution permit on recovery.
use super::*;
use crate::mcp_oauth::McpOAuthOwner;
use crate::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord, check_operation, lock_task};
use crate::team_service::identity_error;
use cowork_identity::{ContentAction, Principal};
use cowork_mcp::{TaskInteractionHandler, TaskServerRequest, TaskServerResponse};
use cowork_secrets::{
    KeyId, KeyPurpose, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, seal_mcp_blob,
};
use futures_util::future::BoxFuture;
use sqlx::{Any, Transaction};
use zeroize::Zeroizing;
fn invalid() -> Error {
    Error::Validation("Invalid original MCP task interaction".into())
}
pub struct McpInteractionRecord {
    pub view: McpTaskInteraction,
    pub task: McpTaskRecord,
    pub operation_id: String,
    pub authority_sha256: String,
    pub request_scope: McpSecretScope,
    pub decision: Option<Value>,
    pub decision_actor: Option<Value>,
}
/// Implemented only by the held original Team/private/workflow execution path.
pub trait McpInteractionExecution: Send + Sync {
    fn validate(&self) -> BoxFuture<'_, Result<()>>;
    fn sample<'a>(&'a self, record: &'a McpInteractionRecord) -> BoxFuture<'a, Result<()>>;
    fn response_bearer(&self) -> BoxFuture<'_, Result<Option<Zeroizing<String>>>> {
        Box::pin(async { Ok(None) })
    }
}
pub struct ScopedTaskInteractions<'a> {
    pub store: &'a Store,
    pub permit: &'a McpTaskOperationPermit,
    pub keys: &'a dyn KeyStore,
    pub key: KeyId,
    pub execution: &'a dyn McpInteractionExecution,
    pub deadline: i64,
}
impl ScopedTaskInteractions<'_> {
    async fn wait(&self, id: &str) -> Result<McpInteractionRecord> {
        loop {
            self.execution.validate().await?;
            let r = self
                .store
                .mcp_task_interaction(self.permit, id, self.keys)
                .await?;
            match r.view.state.as_str() {
                "approved" | "declined" => return Ok(r),
                "awaiting_user" | "awaiting_release" => {}
                _ => return Err(Error::UncertainEffect),
            }
            if now() >= self.deadline {
                return Err(Error::UncertainEffect);
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}
fn transport_error(_: Error) -> cowork_mcp::Error {
    cowork_mcp::Error::SessionUnavailable
}
impl TaskInteractionHandler for ScopedTaskInteractions<'_> {
    fn response_bearer(&self) -> BoxFuture<'_, cowork_mcp::Result<Option<Zeroizing<String>>>> {
        Box::pin(async move {
            self.execution.validate().await.map_err(transport_error)?;
            let result = self
                .execution
                .response_bearer()
                .await
                .map_err(transport_error)?;
            self.execution.validate().await.map_err(transport_error)?;
            Ok(result)
        })
    }
    fn respond<'a>(
        &'a self,
        request: TaskServerRequest,
    ) -> BoxFuture<'a, cowork_mcp::Result<TaskServerResponse>> {
        Box::pin(async move {
            self.execution.validate().await.map_err(transport_error)?;
            let r = self
                .store
                .capture_mcp_task_interaction(
                    self.permit,
                    &request,
                    self.deadline,
                    self.keys,
                    self.key,
                )
                .await
                .map_err(transport_error)?;
            let mut r = self.wait(&r.view.id).await.map_err(transport_error)?;
            if r.view.method == "sampling/createMessage"
                && r.view.state == "approved"
                && r.decision
                    .as_ref()
                    .is_some_and(|d| d["action"] == "approve_sampling")
            {
                self.execution.sample(&r).await.map_err(transport_error)?;
                r = self.wait(&r.view.id).await.map_err(transport_error)?;
            }
            let body = interaction_response(&r).map_err(transport_error)?;
            self.execution.validate().await.map_err(transport_error)?;
            self.store
                .prepare_mcp_interaction_response(
                    self.permit,
                    &r.view.id,
                    &body,
                    self.keys,
                    self.key,
                )
                .await
                .map_err(transport_error)
        })
    }
    fn validate_response<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.execution.validate().await.map_err(transport_error)?;
            self.store
                .validate_mcp_interaction_response(self.permit, response, self.keys)
                .await
                .map_err(transport_error)
        })
    }
    fn acknowledge<'a>(
        &'a self,
        response: &'a TaskServerResponse,
        receipt: Value,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.store
                .finish_mcp_interaction_response(self.permit, response, Some(&receipt))
                .await
                .map_err(transport_error)
        })
    }
    fn unknown<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.store
                .finish_mcp_interaction_response(self.permit, response, None)
                .await
                .map_err(transport_error)
        })
    }
}
fn scope(task: &McpTaskRecord, id: &str, purpose: &str) -> McpSecretScope {
    let mut s = task.scope.clone();
    s.record_id = id.into();
    s.revision = 1;
    s.purpose = purpose.into();
    s
}
fn seal(keys: &dyn KeyStore, key: KeyId, scope: &McpSecretScope, value: &Value) -> Result<String> {
    json_string(
        &seal_mcp_blob(
            keys,
            key,
            scope,
            &Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?),
        )
        .map_err(|_| Error::NativeKeyUnavailable)?,
    )
}
fn open(keys: &dyn KeyStore, scope: &McpSecretScope, raw: &str) -> Result<Value> {
    serde_json::from_slice(
        &open_mcp_blob(keys, scope, &parse::<SealedMcpBlob>(raw)?)
            .map_err(|_| Error::NativeKeyUnavailable)?,
    )
    .map_err(storage_error)
}
pub(crate) async fn record(
    tx: &mut Transaction<'_, Any>,
    id: &str,
    keys: &dyn KeyStore,
) -> Result<McpInteractionRecord> {
    if sqlx::query("UPDATE mcp_task_interactions SET revision=revision WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let r = sqlx::query("SELECT * FROM mcp_task_interactions WHERE id=$1")
        .bind(id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let task = lock_task(
        tx,
        &r.try_get::<String, _>("task_record_id")
            .map_err(storage_error)?,
    )
    .await?;
    let request_scope: McpSecretScope = parse(
        &r.try_get::<String, _>("request_scope")
            .map_err(storage_error)?,
    )?;
    if request_scope != scope(&task, id, "task_interaction_request") {
        return Err(Error::Forbidden);
    }
    let request = open(
        keys,
        &request_scope,
        &r.try_get::<String, _>("sealed_request")
            .map_err(storage_error)?,
    )?;
    let ack = task.open(keys)?;
    let operation_id: String = r.try_get("operation_id").map_err(storage_error)?;
    let rpc: String = sqlx::query_scalar(
        "SELECT rpc_request_id FROM mcp_async_task_operations WHERE id=$1 AND task_record_id=$2",
    )
    .bind(&operation_id)
    .bind(&task.view.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let request_sha256: String = r.try_get("request_sha256").map_err(storage_error)?;
    let method: String = r.try_get("method").map_err(storage_error)?;
    if canonical_hash(&json!([
        task.connection_id,
        task.connection_epoch,
        ack.original.task_id(),
        rpc,
        request
    ])) != request_sha256
        || request["method"] != method
        || json_string(&request["id"])?
            != r.try_get::<String, _>("server_rpc_id")
                .map_err(storage_error)?
        || request
            .pointer("/params/_meta/io.modelcontextprotocol~1related-task/taskId")
            .and_then(Value::as_str)
            != Some(ack.original.task_id())
    {
        return Err(Error::Forbidden);
    }
    let decision_actor = r
        .try_get::<Option<String>, _>("decision_actor")
        .map_err(storage_error)?
        .map(|s| parse::<Value>(&s))
        .transpose()?;
    let decision = r
        .try_get::<Option<String>, _>("sealed_decision")
        .map_err(storage_error)?
        .map(|s| -> Result<Value> {
            let expected = scope(&task, id, "task_interaction_decision");
            if parse::<McpSecretScope>(
                &r.try_get::<String, _>("decision_scope")
                    .map_err(storage_error)?,
            )? != expected
            {
                return Err(Error::Forbidden);
            }
            let envelope = open(keys, &expected, &s)?;
            if envelope["actor"] != decision_actor.clone().ok_or(Error::Forbidden)?
                || envelope["request_sha256"] != request_sha256
                || envelope["operation_id"] != operation_id
            {
                return Err(Error::Forbidden);
            }
            Ok(envelope["answer"].clone())
        })
        .transpose()?;
    let generated_response = r
        .try_get::<Option<String>, _>("sealed_model_response")
        .map_err(storage_error)?
        .map(|s| -> Result<Value> {
            let expected = scope(&task, id, "task_interaction_model_response");
            if parse::<McpSecretScope>(
                &r.try_get::<String, _>("model_response_scope")
                    .map_err(storage_error)?,
            )? != expected
            {
                return Err(Error::Forbidden);
            }
            let response = open(keys, &expected, &s)?;
            if canonical_hash(&response)
                != r.try_get::<String, _>("model_response_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            Ok(response)
        })
        .transpose()?;
    Ok(McpInteractionRecord {
        view: McpTaskInteraction {
            id: id.into(),
            task_record_id: task.view.id.clone(),
            run_id: task.view.run_id.clone(),
            server_id: task.view.server_id.clone(),
            revision: r.try_get("revision").map_err(storage_error)?,
            method,
            state: r.try_get("state").map_err(storage_error)?,
            request_sha256,
            original_request: request,
            generated_response,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            response_sha256: r.try_get("response_sha256").map_err(storage_error)?,
            effects_unknown: true,
        },
        task,
        operation_id,
        authority_sha256: r.try_get("authority_sha256").map_err(storage_error)?,
        request_scope,
        decision,
        decision_actor,
    })
}
async fn actor_proof(tx: &mut Transaction<'_, Any>, actor: &Principal) -> Result<Value> {
    let r=sqlx::query("SELECT revision,expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(now()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    Ok(
        json!({"identity_id":actor.identity_id(),"session_id":actor.session_id(),"session_revision":r.try_get::<i64,_>("revision").map_err(storage_error)?}),
    )
}
fn fingerprint(keys: &dyn KeyStore, key: KeyId, value: &Value) -> Result<String> {
    let material = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = serde_json::to_vec(value).map_err(storage_error)?;
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material.as_ref()),
        &bytes,
    );
    Ok(tag.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}
async fn proof_context(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    proof: &Value,
) -> Result<Value> {
    let session = proof["session_id"].as_str().ok_or(Error::Forbidden)?;
    let identity = proof["identity_id"].as_str().ok_or(Error::Forbidden)?;
    let mut current = if task.owner_kind == "personal" {
        if identity != task.owner_id {
            return Err(Error::Forbidden);
        }
        let r=sqlx::query("SELECT s.revision AS session_revision,p.revision AS project_revision FROM identity_sessions s JOIN identity_subjects i ON i.id=s.identity_id JOIN android_personal_projects p ON p.owner_id=i.id WHERE s.id=$1 AND i.id=$2 AND s.revoked=0 AND s.expires_at>$3 AND i.active=1 AND p.id=$4 AND p.deleted=0 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id)").bind(session).bind(identity).bind(now()).bind(&task.project_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        json!({"identity_id":identity,"session_id":session,"session_revision":r.try_get::<i64,_>("session_revision").map_err(storage_error)?,"project_revision":r.try_get::<i64,_>("project_revision").map_err(storage_error)?})
    } else {
        let r=sqlx::query("SELECT s.revision AS session_revision,o.authorization_epoch,m.id AS membership_id,m.revision AS membership_revision,g.revision AS grant_revision,r.revision AS resource_revision FROM identity_sessions s JOIN identity_subjects i ON i.id=s.identity_id JOIN identity_organizations o ON o.id=$3 JOIN identity_memberships m ON m.identity_id=i.id AND m.organization_id=o.id JOIN identity_resources r ON r.id=$4 AND r.organization_id=o.id JOIN identity_content_grants g ON g.membership_id=m.id AND g.resource_id=r.id WHERE s.id=$1 AND i.id=$2 AND s.revoked=0 AND s.expires_at>$5 AND i.active=1 AND o.state='active' AND m.active=1 AND m.role<>'viewer' AND g.active=1 AND (g.permissions & 5)=5 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id)").bind(session).bind(identity).bind(&task.owner_id).bind(&task.project_id).bind(now()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        json!({"identity_id":identity,"session_id":session,"session_revision":r.try_get::<i64,_>("session_revision").map_err(storage_error)?,"organization_epoch":r.try_get::<i64,_>("authorization_epoch").map_err(storage_error)?,"membership_id":r.try_get::<String,_>("membership_id").map_err(storage_error)?,"membership_revision":r.try_get::<i64,_>("membership_revision").map_err(storage_error)?,"grant_revision":r.try_get::<i64,_>("grant_revision").map_err(storage_error)?,"resource_revision":r.try_get::<i64,_>("resource_revision").map_err(storage_error)?})
    };
    current["owner_kind"] = json!(task.owner_kind);
    current["owner_id"] = json!(task.owner_id);
    current["project_id"] = json!(task.project_id);
    Ok(current)
}
async fn management(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    owner: &McpOAuthOwner<'_>,
    task: &McpTaskRecord,
    write: bool,
) -> Result<Value> {
    if task.owner_kind != owner.kind() || task.owner_id != owner.id() {
        return Err(Error::Forbidden);
    }
    crate::mcp_oauth::evidence_cipher(tx, owner.kind(), owner.key()).await?;
    match owner {
        McpOAuthOwner::Native { key } => {
            let s = crate::native_secrets::lock_state(tx).await?;
            if s.active.as_ref().is_none_or(|v| v.key != *key) || !store.is_local_sqlite() {
                return Err(Error::Forbidden);
            }
            let run: StoredRun = parse(
                &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                    .bind(&task.view.run_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?,
            )?;
            if run.record.spec.project_id != task.project_id
                || canonical_hash(&run.record.spec) != task.spec_sha256
            {
                return Err(Error::Forbidden);
            }
            Ok(json!({"native_instance":key.instance}))
        }
        McpOAuthOwner::Personal { actor, .. } => {
            crate::android_devices::device_writer(tx).await?;
            store
                .android_personal_permit(
                    tx,
                    actor,
                    Uuid::parse_str(&task.project_id).map_err(|_| invalid())?,
                    if write {
                        ContentAction::Write
                    } else {
                        ContentAction::Read
                    },
                )
                .await?;
            let proof = actor_proof(tx, actor).await?;
            proof_context(tx, task, &proof).await
        }
        McpOAuthOwner::Team { actor, team, .. } => {
            let permit = store
                .identity()
                .authorize_content_in_transaction(
                    tx,
                    actor,
                    *team,
                    Uuid::parse_str(&task.project_id).map_err(|_| invalid())?,
                    if write {
                        ContentAction::Write
                    } else {
                        ContentAction::Read
                    },
                )
                .await
                .map_err(identity_error)?;
            store
                .identity()
                .validate_content_in_transaction(tx, &permit)
                .await
                .map_err(identity_error)?;
            let proof = actor_proof(tx, actor).await?;
            if write {
                proof_context(tx, task, &proof).await
            } else {
                Ok(proof)
            }
        }
        _ => Err(Error::Forbidden),
    }
}
/// Revalidate the exact human session/role grant after every wait. This is a
/// read of original provenance, never adoption of a different current session.
async fn decision_current(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    r: &McpInteractionRecord,
) -> Result<()> {
    if r.task.owner_kind == "offline" {
        return crate::offline_mcp_tasks::decision_current(store, tx, r).await;
    }
    let proof = r.decision_actor.as_ref().ok_or(Error::Forbidden)?;
    if r.task.owner_kind == "native" {
        let state = crate::native_secrets::lock_state(tx).await?;
        if proof["native_instance"] != json!(state.instance) {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    if sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::Forbidden);
    }
    let current = proof_context(tx, &r.task, proof).await?;
    if current != *proof {
        return Err(Error::Forbidden);
    }
    let _ = store;
    Ok(())
}
pub(crate) async fn control_current(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    raw: &str,
) -> Result<()> {
    if task.owner_kind == "offline" {
        return crate::offline_mcp_tasks::control_current(tx, task, raw).await;
    }
    if task.owner_kind == "native" {
        let state = crate::native_secrets::lock_state(tx).await?;
        if raw == state.instance.to_string()
            || parse::<Value>(raw).is_ok_and(|v| v["native_instance"] == json!(state.instance))
        {
            return Ok(());
        }
        return Err(Error::Forbidden);
    }
    let proof: Value = parse(raw)?;
    if proof_context(tx, task, &proof).await? != proof {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn scoped_mcp_task_cancel_requested(&self, task: &McpTaskRecord) -> Result<bool> {
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_task_control_requests WHERE task_record_id=$1 AND expected_revision=$2 AND state='requested'").bind(&task.view.id).bind(task.view.revision).fetch_one(&self.pool).await.map_err(storage_error)?;
        Ok(count == 1)
    }
    pub async fn scoped_mcp_tasks(
        &self,
        owner: &McpOAuthOwner<'_>,
        run: &str,
        keys: &dyn KeyStore,
    ) -> Result<Vec<McpAsyncTask>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let rows=sqlx::query("SELECT id FROM mcp_async_tasks WHERE owner_kind=$1 AND owner_id=$2 AND run_id=$3 ORDER BY created_at,id LIMIT 129").bind(owner.kind()).bind(owner.id()).bind(run).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        let mut result = Vec::new();
        for row in rows {
            let task = lock_task(
                &mut tx,
                &row.try_get::<String, _>("id").map_err(storage_error)?,
            )
            .await?;
            management(self, &mut tx, owner, &task, false).await?;
            let _ = task.open(keys)?;
            result.push(task.view);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn scoped_mcp_task_detail(
        &self,
        owner: &McpOAuthOwner<'_>,
        run: &str,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpTaskDetail> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, id).await?;
        if task.view.run_id != run {
            return Err(Error::Forbidden);
        }
        management(self, &mut tx, owner, &task, false).await?;
        let secret = task.open(keys)?;
        let r=sqlx::query("SELECT final_result_scope,sealed_final_result,final_result_sha256 FROM mcp_async_tasks WHERE id=$1").bind(id).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let original_result = if let Some(blob) = r
            .try_get::<Option<String>, _>("sealed_final_result")
            .map_err(storage_error)?
        {
            let s: McpSecretScope = parse(
                &r.try_get::<String, _>("final_result_scope")
                    .map_err(storage_error)?,
            )?;
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_async_task_operations WHERE id=$1 AND task_record_id=$2 AND operation='result' AND state='committed' AND secret_scope=$3").bind(&s.record_id).bind(id).bind(json_string(&s)?).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count != 1
                || s.owner_kind != task.owner_kind
                || s.owner_id != task.owner_id
                || s.project_id != task.project_id
                || s.server_id != task.view.server_id
                || s.configuration_sha256 != task.binding_sha256
                || s.purpose != "task_response"
            {
                return Err(Error::Forbidden);
            }
            let response = open(keys, &s, &blob)?;
            crate::mcp_tasks::result_state(&secret.current, &response)?;
            if canonical_hash(&response["response"])
                != r.try_get::<String, _>("final_result_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            Some(response["response"].clone())
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(McpTaskDetail {
            task: task.view,
            original_result,
        })
    }
    pub async fn request_scoped_mcp_task_cancel(
        &self,
        owner: &McpOAuthOwner<'_>,
        run: &str,
        id: &str,
        input: &McpAsyncTaskRequest,
        _keys: &dyn KeyStore,
    ) -> Result<Value> {
        if input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 128
            || input.idempotency_key.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, id).await?;
        if task.view.run_id != run {
            return Err(Error::Forbidden);
        }
        let actor = management(self, &mut tx, owner, &task, true).await?;
        if let Some(old)=sqlx::query("SELECT expected_revision,original_actor,receipt FROM mcp_task_control_requests WHERE task_record_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if old.try_get::<i64,_>("expected_revision").map_err(storage_error)?!=input.expected_revision||parse::<Value>(&old.try_get::<String,_>("original_actor").map_err(storage_error)?)?!=actor{return Err(Error::Conflict);}let receipt=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt);}
        if input.expected_revision != task.view.revision
            || task.pending_operation_id.is_some()
            || !matches!(task.view.state.as_str(), "working" | "input_required")
            || task.view.expires_at.is_some_and(|v| v <= now())
        {
            return Err(Error::Conflict);
        }
        let receipt = json!({"task_record_id":id,"expected_revision":input.expected_revision,"state":"requested","effects_unknown":true});
        sqlx::query("INSERT INTO mcp_task_control_requests(task_record_id,request_key,expected_revision,operation,state,original_actor,receipt,created_at) VALUES($1,$2,$3,'cancel','requested',$4,$5,$6)").bind(id).bind(&input.idempotency_key).bind(input.expected_revision).bind(json_string(&actor)?).bind(json_string(&receipt)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn capture_mcp_task_interaction(
        &self,
        permit: &McpTaskOperationPermit,
        request: &TaskServerRequest,
        deadline: i64,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpInteractionRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, &permit.task.view.id).await?;
        check_operation(&mut tx, &task, permit).await?;
        crate::mcp_oauth::evidence_cipher(&mut tx, &task.owner_kind, key).await?;
        let operation = sqlx::query("SELECT state FROM mcp_async_task_operations WHERE id=$1")
            .bind(&permit.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if operation
            .try_get::<String, _>("state")
            .map_err(storage_error)?
            != "admitted"
            || request.connection_id() != task.connection_id
            || request.connection_epoch() != task.connection_epoch
            || request.operation_rpc_id() != permit.rpc_request_id
            || canonical_hash(&request.task_id()) != task.task_id_sha256
            || deadline <= now()
        {
            return Err(Error::Forbidden);
        }
        let rpc = json_string(request.rpc_id())?;
        if sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM mcp_task_interactions WHERE task_record_id=$1",
        )
        .bind(&task.view.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?
            >= 128
        {
            return Err(Error::BudgetExceeded);
        }
        // A repeated server request is never another send opportunity.
        if sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM mcp_task_interactions WHERE task_record_id=$1 AND server_rpc_id=$2").bind(&task.view.id).bind(&rpc).fetch_one(&mut *tx).await.map_err(storage_error)?!=0{return Err(Error::UncertainEffect);}
        let id = Uuid::new_v4().to_string();
        let s = scope(&task, &id, "task_interaction_request");
        let expires = deadline.min(task.view.expires_at.unwrap_or(i64::MAX));
        sqlx::query("INSERT INTO mcp_task_interactions(id,task_record_id,operation_id,server_rpc_id,method,request_sha256,authority_sha256,revision,state,request_scope,sealed_request,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,1,'awaiting_user',$8,$9,$10,$11,$11)").bind(&id).bind(&task.view.id).bind(&permit.id).bind(&rpc).bind(request.method()).bind(request.request_sha256()).bind(&permit.authority_sha256).bind(json_string(&s)?).bind(seal(keys,key,&s,request.wire())?).bind(expires).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        let result = record(&mut tx, &id, keys).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn mcp_task_interaction(
        &self,
        permit: &McpTaskOperationPermit,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpInteractionRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, id, keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        if r.operation_id != permit.id
            || r.authority_sha256 != permit.authority_sha256
            || r.view.expires_at <= now()
        {
            return Err(Error::Forbidden);
        }
        if r.decision_actor.is_some() {
            decision_current(self, &mut tx, &r).await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(r)
    }
    pub async fn mcp_task_interactions(
        &self,
        owner: &McpOAuthOwner<'_>,
        run: &str,
        keys: &dyn KeyStore,
    ) -> Result<Vec<McpTaskInteraction>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let rows=sqlx::query("SELECT i.id FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 AND t.owner_id=$2 AND t.run_id=$3 ORDER BY i.created_at,i.id LIMIT 129").bind(owner.kind()).bind(owner.id()).bind(run).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        let mut result = Vec::new();
        for row in rows {
            let r = record(
                &mut tx,
                &row.try_get::<String, _>("id").map_err(storage_error)?,
                keys,
            )
            .await?;
            management(self, &mut tx, owner, &r.task, false).await?;
            result.push(r.view);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn answer_mcp_task_interaction(
        &self,
        owner: &McpOAuthOwner<'_>,
        run: &str,
        id: &str,
        input: &AnswerMcpTaskInteraction,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        if input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 128
            || input.idempotency_key.chars().any(char::is_control)
            || input.expected_revision <= 0
            || serde_json::to_vec(input).map_err(storage_error)?.len() > 128 * 1024
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, id, keys).await?;
        if r.task.view.run_id != run {
            return Err(Error::Forbidden);
        }
        let actor = management(self, &mut tx, owner, &r.task, true).await?;
        if let Some(old)=sqlx::query("SELECT * FROM mcp_task_interaction_decisions WHERE interaction_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{let original:KeyId=parse(&old.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;let digest=fingerprint(keys,original,&json!([run,id,input,actor]))?;if digest!=old.try_get::<String,_>("request_sha256").map_err(storage_error)?{return Err(Error::Conflict);}let receipt=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt);}
        if r.view.revision != input.expected_revision
            || r.view.expires_at <= now()
            || r.task.pending_operation_id.as_deref() != Some(&r.operation_id)
        {
            return Err(Error::Conflict);
        }
        let admitted:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_async_task_operations WHERE id=$1 AND task_record_id=$2 AND expected_revision=$3 AND state='admitted'").bind(&r.operation_id).bind(&r.task.view.id).bind(r.task.view.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if admitted != 1 {
            return Err(Error::UncertainEffect);
        }
        let newstate = if r.view.method == "elicitation/create" && r.view.state == "awaiting_user" {
            if !matches!(input.action.as_str(), "accept" | "decline" | "cancel")
                || input.edited_messages.is_some()
            {
                return Err(invalid());
            }
            if input.action == "accept" {
                validate_form(
                    &r.view.original_request["params"],
                    input.content.as_ref().ok_or_else(invalid)?,
                )?;
            } else if input.content.is_some() {
                return Err(invalid());
            }
            if input.action == "accept" {
                "approved"
            } else {
                "declined"
            }
        } else if r.view.method == "sampling/createMessage" && r.view.state == "awaiting_user" {
            if !matches!(input.action.as_str(), "approve_sampling" | "decline_sample")
                || input.content.is_some()
            {
                return Err(invalid());
            }
            if input.action == "approve_sampling" {
                sampling_messages(
                    &r.view.original_request["params"],
                    input.edited_messages.as_deref(),
                )?;
                "approved"
            } else {
                "declined"
            }
        } else if r.view.method == "sampling/createMessage" && r.view.state == "awaiting_release" {
            if !matches!(input.action.as_str(), "release_sample" | "decline_sample")
                || input.content.is_some()
                || input.edited_messages.is_some()
            {
                return Err(invalid());
            }
            if input.action == "release_sample" {
                "approved"
            } else {
                "declined"
            }
        } else {
            return Err(Error::Conflict);
        };
        let authkey = keys
            .key_for_purpose(&owner.key(), KeyPurpose::RequestFingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let digest = fingerprint(keys, authkey, &json!([run, id, input, actor]))?;
        let s = scope(&r.task, id, "task_interaction_decision");
        let receipt = json!({"interaction_id":id,"task_record_id":r.task.view.id,"revision":r.view.revision+1,"state":newstate,"decision":input.action,"request_sha256":r.view.request_sha256,"effects_unknown":true});
        let envelope = json!({"answer":input,"actor":actor,"request_sha256":r.view.request_sha256,"operation_id":r.operation_id});
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state=$1,decision_scope=$2,sealed_decision=$3,decision_actor=$4,updated_at=$5 WHERE id=$6 AND revision=$7 AND state=$8").bind(newstate).bind(json_string(&s)?).bind(seal(keys,owner.key(),&s,&envelope)?).bind(json_string(&actor)?).bind(now()).bind(id).bind(input.expected_revision).bind(&r.view.state).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        let original_scope = scope(
            &r.task,
            &format!(
                "decision:{}",
                canonical_hash(&json!([id, input.idempotency_key]))
            ),
            "task_interaction_decision",
        );
        sqlx::query("INSERT INTO mcp_task_interaction_decisions(interaction_id,request_key,expected_revision,request_sha256,request_key_id,secret_scope,sealed_decision,original_actor,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(id).bind(&input.idempotency_key).bind(input.expected_revision).bind(digest).bind(json_string(&authkey)?).bind(json_string(&original_scope)?).bind(seal(keys,owner.key(),&original_scope,&envelope)?).bind(json_string(&actor)?).bind(json_string(&receipt)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn admit_mcp_interaction_sampling(
        &self,
        permit: &McpTaskOperationPermit,
        id: &str,
        intent: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, id, keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        crate::mcp_oauth::evidence_cipher(&mut tx, &r.task.owner_kind, key).await?;
        decision_current(self, &mut tx, &r).await?;
        if r.operation_id != permit.id
            || r.authority_sha256 != permit.authority_sha256
            || r.view.state != "approved"
            || r.view.method != "sampling/createMessage"
            || r.decision
                .as_ref()
                .is_none_or(|d| d["action"] != "approve_sampling")
            || r.view.expires_at <= now()
        {
            return Err(Error::Forbidden);
        }
        let attempt = format!("mcp-sampling:{id}");
        let s = scope(&r.task, id, "task_interaction_model_intent");
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state='sampling_admitted',model_attempt_id=$1,model_intent_scope=$2,sealed_model_intent=$3,model_request_sha256=$4,updated_at=$5 WHERE id=$6 AND revision=$7 AND model_attempt_id IS NULL").bind(&attempt).bind(json_string(&s)?).bind(seal(keys,key,&s,intent)?).bind(canonical_hash(intent)).bind(now()).bind(id).bind(r.view.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect);}
        tx.commit().await.map_err(storage_error)
    }
    /// Evidence capture survives actor revocation; it does not release a sample.
    pub async fn capture_mcp_interaction_sampling(
        &self,
        permit: &McpTaskOperationPermit,
        id: &str,
        response: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, id, keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        crate::mcp_oauth::evidence_cipher(&mut tx, &r.task.owner_kind, key).await?;
        if r.operation_id != permit.id || r.view.state != "sampling_admitted" {
            return Err(Error::UncertainEffect);
        }
        let s = scope(&r.task, id, "task_interaction_model_response");
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state=$6,model_response_scope=$1,sealed_model_response=$2,model_response_sha256=$3,updated_at=$4 WHERE id=$5 AND state='sampling_admitted' AND sealed_model_response IS NULL").bind(json_string(&s)?).bind(seal(keys,key,&s,response)?).bind(canonical_hash(response)).bind(now()).bind(id).bind(if response["result"].is_object(){"awaiting_release"}else{"unknown"}).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)
    }
    pub async fn prepare_mcp_interaction_response(
        &self,
        permit: &McpTaskOperationPermit,
        id: &str,
        body: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<TaskServerResponse> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, id, keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        crate::mcp_oauth::evidence_cipher(&mut tx, &r.task.owner_kind, key).await?;
        decision_current(self, &mut tx, &r).await?;
        if r.operation_id != permit.id
            || r.authority_sha256 != permit.authority_sha256
            || !matches!(r.view.state.as_str(), "approved" | "declined")
            || r.view.expires_at <= now()
        {
            return Err(Error::Forbidden);
        }
        let expected = interaction_response(&r)?;
        if &expected != body {
            return Err(Error::Forbidden);
        }
        let s = scope(&r.task, id, "task_interaction_response");
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state='response_admitted',response_scope=$1,sealed_response=$2,response_sha256=$3,updated_at=$4 WHERE id=$5 AND revision=$6 AND sealed_response IS NULL").bind(json_string(&s)?).bind(seal(keys,key,&s,body)?).bind(canonical_hash(body)).bind(now()).bind(id).bind(r.view.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect);}
        tx.commit().await.map_err(storage_error)?;
        TaskServerResponse::prepared(id.into(), body.clone()).map_err(|_| invalid())
    }
    pub async fn validate_mcp_interaction_response(
        &self,
        permit: &McpTaskOperationPermit,
        response: &TaskServerResponse,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = record(&mut tx, response.receipt_id(), keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        decision_current(self, &mut tx, &r).await?;
        if r.view.state != "response_admitted"
            || r.operation_id != permit.id
            || r.authority_sha256 != permit.authority_sha256
            || r.view.expires_at <= now()
            || r.view.response_sha256.as_deref() != Some(canonical_hash(response.body()).as_str())
        {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn finish_mcp_interaction_response(
        &self,
        permit: &McpTaskOperationPermit,
        response: &TaskServerResponse,
        receipt: Option<&Value>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, &permit.task.view.id).await?;
        check_operation(&mut tx, &task, permit).await?;
        let original = json!({"interaction_id":response.receipt_id(),"task_record_id":task.view.id,"operation_id":permit.id,"response_sha256":canonical_hash(response.body()),"transport_receipt":receipt,"state":if receipt.is_some(){"responded"}else{"unknown"},"remote_effects_unknown":true});
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state=$1,receipt=$2,updated_at=$3 WHERE id=$4 AND task_record_id=$5 AND operation_id=$6 AND state='response_admitted' AND response_sha256=$7 AND receipt IS NULL").bind(if receipt.is_some(){"responded"}else{"unknown"}).bind(json_string(&original)?).bind(now()).bind(response.receipt_id()).bind(&task.view.id).bind(&permit.id).bind(canonical_hash(response.body())).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect);}
        tx.commit().await.map_err(storage_error)
    }
}
pub fn sampling_messages(params: &Value, edited: Option<&[Value]>) -> Result<Vec<Value>> {
    if params.get("tools").is_some()
        || params.get("toolChoice").is_some()
        || params.get("includeContext").is_some_and(|v| v != "none")
        || params["maxTokens"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > 1_000_000)
    {
        return Err(invalid());
    }
    let messages = edited.unwrap_or(params["messages"].as_array().ok_or_else(invalid)?);
    if messages.is_empty() || messages.len() > 128 {
        return Err(invalid());
    }
    let mut result = Vec::new();
    if let Some(prompt) = params.get("systemPrompt") {
        let s = prompt.as_str().ok_or_else(invalid)?;
        if s.len() > 32768 {
            return Err(invalid());
        }
        result.push(json!({"role":"user","content":format!("MCP server's requested guidance (reviewed as untrusted input): {s}")}));
    }
    for m in messages {
        let role = m["role"]
            .as_str()
            .filter(|s| matches!(*s, "user" | "assistant"))
            .ok_or_else(invalid)?;
        let parts = if m["content"].is_array() {
            m["content"].as_array().ok_or_else(invalid)?.clone()
        } else {
            vec![m["content"].clone()]
        };
        if parts.is_empty() || parts.len() > 32 {
            return Err(invalid());
        }
        let mut text = String::new();
        for part in parts {
            if part["type"] != "text" {
                return Err(Error::ContextLimit);
            }
            let s = part["text"].as_str().ok_or_else(invalid)?;
            if text.len() + s.len() > 65536 {
                return Err(Error::ContextLimit);
            }
            text.push_str(s);
            text.push('\n');
        }
        result.push(json!({"role":role,"content":text}));
    }
    if json_string(&result)?.len() > 128 * 1024 {
        return Err(Error::ContextLimit);
    }
    Ok(result)
}
pub fn validate_form(params: &Value, content: &Value) -> Result<()> {
    if params.get("mode").is_some_and(|v| v != "form")
        || params["message"].as_str().is_none_or(|v| v.len() > 32768)
        || !content.is_object()
    {
        return Err(invalid());
    }
    let schema = &params["requestedSchema"];
    if schema["type"] != "object"
        || schema.as_object().is_none_or(|s| {
            s.keys().any(|k| {
                ![
                    "type",
                    "properties",
                    "required",
                    "title",
                    "description",
                    "additionalProperties",
                ]
                .contains(&k.as_str())
            })
        })
    {
        return Err(invalid());
    }
    let properties = schema["properties"].as_object().ok_or_else(invalid)?;
    if properties.len() > 64 || json_string(schema)?.len() > 32768 {
        return Err(invalid());
    }
    if schema.get("required").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 64
                || a.iter()
                    .any(|v| v.as_str().is_none_or(|k| !properties.contains_key(k)))
        })
    }) {
        return Err(invalid());
    }
    for (name, p) in properties {
        if name.is_empty()
            || name.len() > 128
            || !p.is_object()
            || !matches!(
                p["type"].as_str(),
                Some("string" | "number" | "integer" | "boolean" | "array")
            )
            || p.as_object().is_some_and(|s| {
                s.keys().any(|k| {
                    ![
                        "type",
                        "title",
                        "description",
                        "default",
                        "enum",
                        "enumNames",
                        "minLength",
                        "maxLength",
                        "pattern",
                        "format",
                        "minimum",
                        "maximum",
                        "exclusiveMinimum",
                        "exclusiveMaximum",
                        "multipleOf",
                        "items",
                        "minItems",
                        "maxItems",
                        "uniqueItems",
                    ]
                    .contains(&k.as_str())
                })
            })
        {
            return Err(invalid());
        }
        if p["type"] == "array"
            && (p["items"]["type"] != "string"
                || !p["items"]["enum"].is_array()
                || p["items"].as_object().is_none_or(|s| {
                    s.keys()
                        .any(|k| !["type", "enum", "title", "description"].contains(&k.as_str()))
                }))
        {
            return Err(invalid());
        }
        for values in [p.get("enum"), p.pointer("/items/enum")]
            .into_iter()
            .flatten()
        {
            if values.as_array().is_none_or(|v| {
                v.len() > 128
                    || v.iter()
                        .any(|v| !v.is_string() || v.as_str().is_some_and(|s| s.len() > 4096))
            }) {
                return Err(invalid());
            }
        }
        if p.get("pattern")
            .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 256))
        {
            return Err(invalid());
        }
    }
    if content
        .as_object()
        .is_some_and(|v| v.keys().any(|k| !properties.contains_key(k)))
        || json_string(content)?.len() > 65536
    {
        return Err(invalid());
    }
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|_| invalid())?;
    if !validator.is_valid(content) {
        return Err(invalid());
    }
    Ok(())
}
pub fn interaction_response(r: &McpInteractionRecord) -> Result<Value> {
    let decision = r.decision.as_ref().ok_or(Error::Forbidden)?;
    let meta = json!({"io.modelcontextprotocol/related-task":{"taskId":r.view.original_request.pointer("/params/_meta/io.modelcontextprotocol~1related-task/taskId").and_then(Value::as_str).ok_or(Error::Forbidden)?}});
    if r.view.method == "elicitation/create" {
        let action = decision["action"].as_str().ok_or_else(invalid)?;
        if !matches!(action, "accept" | "decline" | "cancel") {
            return Err(invalid());
        }
        let mut result = json!({"action":action,"_meta":meta});
        if action == "accept" {
            validate_form(&r.view.original_request["params"], &decision["content"])?;
            result["content"] = decision["content"].clone();
        }
        return Ok(json!({"result":result}));
    }
    if decision["action"] == "decline_sample" {
        return Ok(
            json!({"error":{"code":-1,"message":"User declined sampling","data":{"_meta":meta}}}),
        );
    }
    if decision["action"] != "release_sample" {
        return Err(Error::Forbidden);
    }
    let generated = r
        .view
        .generated_response
        .as_ref()
        .ok_or(Error::UncertainEffect)?;
    let mut result = generated["result"].clone();
    if !result.is_object() {
        return Err(Error::Forbidden);
    }
    result["_meta"] = meta;
    Ok(json!({"result":result}))
}
pub(crate) async fn required_keys(tx: &mut Transaction<'_, Any>, kind: &str) -> Result<Vec<KeyId>> {
    let mut result = Vec::new();
    for column in [
        "sealed_request",
        "sealed_decision",
        "sealed_model_intent",
        "sealed_model_response",
        "sealed_response",
    ] {
        let query = format!(
            "SELECT i.{column} AS blob FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 AND i.{column} IS NOT NULL ORDER BY i.id LIMIT 100001"
        );
        let rows = sqlx::query(&query)
            .bind(kind)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if rows.len() > 100_000 {
            return Err(Error::BudgetExceeded);
        }
        for row in rows {
            let blob: SealedMcpBlob =
                parse(&row.try_get::<String, _>("blob").map_err(storage_error)?)?;
            blob.validate().map_err(|_| Error::Forbidden)?;
            if !result.contains(&blob.key_id()) {
                result.push(blob.key_id());
            }
        }
    }
    let rows=sqlx::query("SELECT d.request_key_id,d.sealed_decision FROM mcp_task_interaction_decisions d JOIN mcp_task_interactions i ON i.id=d.interaction_id JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 LIMIT 100001").bind(kind).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    for row in rows {
        let key: KeyId = parse(
            &row.try_get::<String, _>("request_key_id")
                .map_err(storage_error)?,
        )?;
        key.validate().map_err(|_| Error::Forbidden)?;
        let blob: SealedMcpBlob = parse(
            &row.try_get::<String, _>("sealed_decision")
                .map_err(storage_error)?,
        )?;
        blob.validate().map_err(|_| Error::Forbidden)?;
        for key in [key, blob.key_id()] {
            if !result.contains(&key) {
                result.push(key);
            }
        }
    }
    Ok(result)
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
    kind: &str,
) -> Result<u64> {
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 AND i.state IN('sampling_admitted','response_admitted')").bind(kind).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 {
        return Err(Error::Conflict);
    }
    let mut count = 0;
    for (column, scope_column, purpose) in [
        (
            "sealed_request",
            "request_scope",
            "task_interaction_request",
        ),
        (
            "sealed_decision",
            "decision_scope",
            "task_interaction_decision",
        ),
        (
            "sealed_model_intent",
            "model_intent_scope",
            "task_interaction_model_intent",
        ),
        (
            "sealed_model_response",
            "model_response_scope",
            "task_interaction_model_response",
        ),
        (
            "sealed_response",
            "response_scope",
            "task_interaction_response",
        ),
    ] {
        let query = format!(
            "SELECT i.id,i.{column} AS blob,i.{scope_column} AS scope,t.owner_id,t.project_id,t.server_id,t.binding_sha256 FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 AND i.{column} IS NOT NULL ORDER BY i.id LIMIT 100001"
        );
        let rows = sqlx::query(&query)
            .bind(kind)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if rows.len() > 100_000 {
            return Err(Error::BudgetExceeded);
        }
        for row in rows {
            let id: String = row.try_get("id").map_err(storage_error)?;
            let s: McpSecretScope =
                parse(&row.try_get::<String, _>("scope").map_err(storage_error)?)?;
            let original: String = row.try_get("blob").map_err(storage_error)?;
            let blob: SealedMcpBlob = parse(&original)?;
            if s.owner_kind != kind
                || s.owner_id
                    != row
                        .try_get::<String, _>("owner_id")
                        .map_err(storage_error)?
                || s.project_id
                    != row
                        .try_get::<String, _>("project_id")
                        .map_err(storage_error)?
                || s.server_id
                    != row
                        .try_get::<String, _>("server_id")
                        .map_err(storage_error)?
                || s.configuration_sha256
                    != row
                        .try_get::<String, _>("binding_sha256")
                        .map_err(storage_error)?
                || s.record_id != id
                || s.revision != 1
                || s.purpose != purpose
                || blob.key_id().instance != target.instance
            {
                return Err(Error::Forbidden);
            }
            let updated = cowork_secrets::rewrap_mcp_blob(keys, &s, &blob, target)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            if updated == blob {
                continue;
            }
            let update = format!(
                "UPDATE mcp_task_interactions SET {column}=$1 WHERE id=$2 AND {column}=$3 AND {scope_column}=$4"
            );
            if sqlx::query(&update)
                .bind(json_string(&updated)?)
                .bind(&id)
                .bind(&original)
                .bind(json_string(&s)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?
                .rows_affected()
                != 1
            {
                return Err(Error::Conflict);
            }
            count += 1;
        }
    }
    let rows=sqlx::query("SELECT d.*,i.operation_id,i.request_sha256 AS original_server_sha,t.owner_id,t.project_id,t.server_id,t.binding_sha256 FROM mcp_task_interaction_decisions d JOIN mcp_task_interactions i ON i.id=d.interaction_id JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind=$1 ORDER BY d.interaction_id,d.request_key LIMIT 100001").bind(kind).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    for row in rows {
        let interaction: String = row.try_get("interaction_id").map_err(storage_error)?;
        let request_key: String = row.try_get("request_key").map_err(storage_error)?;
        let s: McpSecretScope = parse(
            &row.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )?;
        let original: String = row.try_get("sealed_decision").map_err(storage_error)?;
        let blob: SealedMcpBlob = parse(&original)?;
        if s.owner_kind != kind
            || s.owner_id
                != row
                    .try_get::<String, _>("owner_id")
                    .map_err(storage_error)?
            || s.project_id
                != row
                    .try_get::<String, _>("project_id")
                    .map_err(storage_error)?
            || s.server_id
                != row
                    .try_get::<String, _>("server_id")
                    .map_err(storage_error)?
            || s.configuration_sha256
                != row
                    .try_get::<String, _>("binding_sha256")
                    .map_err(storage_error)?
            || s.record_id
                != format!(
                    "decision:{}",
                    canonical_hash(&json!([interaction, request_key]))
                )
            || s.revision != 1
            || s.purpose != "task_interaction_decision"
            || blob.key_id().instance != target.instance
        {
            return Err(Error::Forbidden);
        }
        let envelope: Value = serde_json::from_slice(
            &open_mcp_blob(keys, &s, &blob).map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if envelope["actor"]
            != parse::<Value>(
                &row.try_get::<String, _>("original_actor")
                    .map_err(storage_error)?,
            )?
            || envelope["operation_id"]
                != row
                    .try_get::<String, _>("operation_id")
                    .map_err(storage_error)?
            || envelope["request_sha256"]
                != row
                    .try_get::<String, _>("original_server_sha")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let updated = cowork_secrets::rewrap_mcp_blob(keys, &s, &blob, target)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if updated == blob {
            continue;
        }
        if sqlx::query("UPDATE mcp_task_interaction_decisions SET sealed_decision=$1 WHERE interaction_id=$2 AND request_key=$3 AND sealed_decision=$4 AND secret_scope=$5").bind(json_string(&updated)?).bind(&interaction).bind(&request_key).bind(&original).bind(json_string(&s)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        count += 1;
    }
    Ok(count)
}
