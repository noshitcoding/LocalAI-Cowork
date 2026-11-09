use super::source::*;
use super::*;
use crate::mcp_interactions::{McpInteractionRecord, record, sampling_messages, validate_form};
use crate::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord, check_operation, lock_task};
use cowork_secrets::{KeyPurpose, KeyStore, McpSecretScope};
fn interaction_scope(t: &McpTaskRecord, id: &str, purpose: &str) -> McpSecretScope {
    let mut s = t.scope.clone();
    s.record_id = id.into();
    s.revision = 1;
    s.purpose = purpose.into();
    s
}
fn fingerprint(keys: &dyn KeyStore, key: KeyId, value: &Value) -> Result<String> {
    let bytes = keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, bytes.as_ref()),
        &serde_json::to_vec(value).map_err(storage_error)?,
    );
    Ok(tag.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}
fn request_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
/// Decision proofs are authenticated by the checked interaction envelope. This
/// rechecks its exact original source locators against the current local ticket.
async fn proof_current(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    proof_value: &Value,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT * FROM offline_mcp_calls WHERE task_record_id=$1 AND run_id=$2 AND device_id=$3",
    )
    .bind(&task.view.id)
    .bind(&task.view.run_id)
    .bind(&task.owner_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::Forbidden)?;
    let ticket: SignedOfflineGrant = parse(
        &sqlx::query_scalar::<_, String>("SELECT ticket FROM offline_local_grants WHERE id=$1")
            .bind(
                row.try_get::<String, _>("grant_id")
                    .map_err(storage_error)?,
            )
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )?;
    crate::offline_execution::validate_offline_ticket(&ticket)?;
    let run: OfflineRun = parse(
        &sqlx::query_scalar::<_, String>("SELECT payload FROM offline_local_runs WHERE id=$1")
            .bind(&task.view.run_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )?;
    let tool = run.pending_tools.first().ok_or(Error::Forbidden)?.clone();
    let cfg = configuration(tx, ticket.grant.id).await?;
    let binding = cfg
        .mcp_connections
        .iter()
        .find(|b| b.id == task.view.server_id)
        .ok_or(Error::Forbidden)?;
    let s = Source {
        format: 1,
        id: row.try_get("id").map_err(storage_error)?,
        ticket,
        run: run.id,
        ordinal: run.ordinal,
        tool,
        configuration_sha256: row.try_get("configuration_sha256").map_err(storage_error)?,
        endpoint: binding.endpoint.clone(),
        server_id: binding.id.clone(),
        catalog_sha256: binding.catalog_sha256.clone(),
        connection_id: task.connection_id.clone(),
        connection_epoch: task.connection_epoch,
        original_rpc_id: task.original_rpc_id.clone(),
        transport_request_sha256: task.transport_request_sha256.clone(),
        protocol: MCP_PROTOCOL_VERSION.into(),
        task_ttl_ms: None,
        deadline: run.deadline_at,
    };
    let p = OfflineMcpCallPermit {
        source: s,
        source_sha256: row.try_get("source_sha256").map_err(storage_error)?,
    };
    if task.owner_kind != "offline"
        || *proof_value != proof(&p)
        || row.try_get::<String, _>("state").map_err(storage_error)? != "received"
        || row
            .try_get::<i64, _>("dispatch_admitted")
            .map_err(storage_error)?
            != 1
        || p.source.tool.id != row.try_get::<String, _>("tool_id").map_err(storage_error)?
        || p.source.tool.request_sha256
            != row
                .try_get::<String, _>("tool_request_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    crate::mcp_scoped_tasks::check(&facts(&p), task)?;
    current(tx, &p.source, true).await?;
    Ok(())
}
pub(crate) async fn decision_current(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    r: &McpInteractionRecord,
) -> Result<()> {
    if !store.is_local_sqlite()
        || r.decision_actor
            .as_ref()
            .is_none_or(|p| p["source_sha256"] != r.authority_sha256)
    {
        return Err(Error::Forbidden);
    }
    proof_current(
        tx,
        &r.task,
        r.decision_actor.as_ref().ok_or(Error::Forbidden)?,
    )
    .await
}
pub(crate) async fn control_current(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    raw: &str,
) -> Result<()> {
    proof_current(tx, task, &parse::<Value>(raw)?).await
}
impl Store {
    pub async fn offline_mcp_tasks(
        &self,
        run: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Vec<McpAsyncTask>> {
        if !self.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let rows=sqlx::query("SELECT id FROM mcp_async_tasks WHERE owner_kind='offline' AND run_id=$1 ORDER BY created_at,id LIMIT 129").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
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
            task_source(&mut tx, &task, keys, false).await?;
            task.open(keys)?;
            result.push(task.view)
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn offline_mcp_task_detail(
        &self,
        run: Uuid,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpTaskDetail> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, id).await?;
        if task.view.run_id != run.to_string() {
            return Err(Error::Forbidden);
        }
        let p = task_source(&mut tx, &task, keys, false).await?;
        task.open(keys)?;
        let has_result = task.view.result_sha256.is_some();
        tx.commit().await.map_err(storage_error)?;
        let original_result = if has_result {
            Some(
                self.offline_mcp_original_result(run, &p.source.tool, keys)
                    .await?["response"]
                    .clone(),
            )
        } else {
            None
        };
        Ok(McpTaskDetail {
            task: task.view,
            original_result,
        })
    }
    pub async fn offline_mcp_interactions(
        &self,
        run: Uuid,
        after: Option<&str>,
        only_waiting: bool,
        keys: &dyn KeyStore,
    ) -> Result<OfflineMcpInteractionPage> {
        if !self.is_local_sqlite() || after.is_some_and(|s| Uuid::parse_str(s).is_err()) {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let rows=sqlx::query("SELECT i.id FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind='offline' AND t.run_id=$1 AND i.id>$2 AND ($3=0 OR i.state IN('awaiting_user','awaiting_release')) ORDER BY i.id LIMIT 3").bind(run.to_string()).bind(after.unwrap_or("")).bind(i64::from(only_waiting)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > 2;
        let mut items = Vec::new();
        for row in rows.into_iter().take(2) {
            let r = record(
                &mut tx,
                &row.try_get::<String, _>("id").map_err(storage_error)?,
                keys,
            )
            .await?;
            task_source(&mut tx, &r.task, keys, false).await?;
            items.push(r.view)
        }
        let next_cursor = if more {
            items.last().map(|r| r.id.clone())
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(OfflineMcpInteractionPage { items, next_cursor })
    }
    pub async fn cancel_offline_mcp_task(
        &self,
        run: Uuid,
        id: &str,
        input: &McpAsyncTaskRequest,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        request_key(&input.idempotency_key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let task = lock_task(&mut tx, id).await?;
        if task.view.run_id != run.to_string() {
            return Err(Error::Forbidden);
        }
        let p = task_source(&mut tx, &task, keys, false).await?;
        let actor = proof(&p);
        if let Some(old)=sqlx::query("SELECT expected_revision,original_actor,receipt FROM mcp_task_control_requests WHERE task_record_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if old.try_get::<i64,_>("expected_revision").map_err(storage_error)?!=input.expected_revision||parse::<Value>(&old.try_get::<String,_>("original_actor").map_err(storage_error)?)?!=actor{return Err(Error::Conflict)}let receipt=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt)}
        current(&mut tx, &p.source, true).await?;
        if input.expected_revision != task.view.revision
            || task.pending_operation_id.is_some()
            || !matches!(task.view.state.as_str(), "working" | "input_required")
            || task.view.expires_at.is_some_and(|v| v <= now())
        {
            return Err(Error::Conflict);
        }
        let receipt = json!({"task_record_id":id,"expected_revision":input.expected_revision,"state":"requested","effects_unknown":true});
        sqlx::query("INSERT INTO mcp_task_control_requests(task_record_id,request_key,expected_revision,operation,state,original_actor,receipt,created_at) VALUES($1,$2,$3,'cancel','requested',$4,$5,$6)").bind(id).bind(&input.idempotency_key).bind(input.expected_revision).bind(json_string(&actor)?).bind(json_string(&receipt)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn answer_offline_mcp_interaction(
        &self,
        run: Uuid,
        id: &str,
        input: &AnswerMcpTaskInteraction,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        request_key(&input.idempotency_key)?;
        if input.expected_revision <= 0
            || serde_json::to_vec(input).map_err(storage_error)?.len() > 128 * 1024
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        let r = record(&mut tx, id, keys).await?;
        if r.task.view.run_id != run.to_string() {
            return Err(Error::Forbidden);
        }
        let p = task_source(&mut tx, &r.task, keys, false).await?;
        let actor = proof(&p);
        if let Some(old)=sqlx::query("SELECT * FROM mcp_task_interaction_decisions WHERE interaction_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            let auth:KeyId=parse(&old.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;
            if auth.instance!=key.instance||old.try_get::<i64,_>("expected_revision").map_err(storage_error)?!=input.expected_revision||fingerprint(keys,auth,&json!([run.to_string(),id,input,actor]))?!=old.try_get::<String,_>("request_sha256").map_err(storage_error)?{return Err(Error::Conflict)}
            let aad=interaction_scope(&r.task,&format!("decision:{}",canonical_hash(&json!([id,input.idempotency_key]))),"task_interaction_decision");
            if parse::<McpSecretScope>(&old.try_get::<String,_>("secret_scope").map_err(storage_error)?)?!=aad{return Err(Error::Forbidden)}
            let envelope=open(keys,&aad,&old.try_get::<String,_>("sealed_decision").map_err(storage_error)?)?;
            if envelope["answer"]!=serde_json::to_value(input).map_err(storage_error)?||envelope["actor"]!=actor||envelope["request_sha256"]!=r.view.request_sha256||envelope["operation_id"]!=r.operation_id{return Err(Error::Forbidden)}
            let state=if matches!(input.action.as_str(),"accept"|"approve_sampling"|"release_sample"){"approved"}else{"declined"};
            let expected=json!({"interaction_id":id,"task_record_id":r.task.view.id,"revision":input.expected_revision+1,"state":state,"decision":input.action,"request_sha256":r.view.request_sha256,"effects_unknown":true});
            let receipt:Value=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;if receipt!=expected{return Err(Error::Forbidden)}
            tx.commit().await.map_err(storage_error)?;return Ok(receipt)
        }
        current(&mut tx, &p.source, true).await?;
        if r.view.revision != input.expected_revision
            || r.view.expires_at <= now()
            || r.task.pending_operation_id.as_deref() != Some(r.operation_id.as_str())
            || r.authority_sha256 != p.source_sha256
        {
            return Err(Error::Conflict);
        }
        if sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM mcp_async_task_operations WHERE id=$1 AND task_record_id=$2 AND expected_revision=$3 AND state='admitted'").bind(&r.operation_id).bind(&r.task.view.id).bind(r.task.view.revision).fetch_one(&mut *tx).await.map_err(storage_error)?!=1{return Err(Error::UncertainEffect)}
        let state = match (
            r.view.method.as_str(),
            r.view.state.as_str(),
            input.action.as_str(),
        ) {
            ("elicitation/create", "awaiting_user", "accept") => {
                if input.edited_messages.is_some() {
                    return Err(Error::Forbidden);
                }
                validate_form(
                    &r.view.original_request["params"],
                    input.content.as_ref().ok_or(Error::Forbidden)?,
                )?;
                "approved"
            }
            ("elicitation/create", "awaiting_user", "decline" | "cancel") => {
                if input.content.is_some() || input.edited_messages.is_some() {
                    return Err(Error::Forbidden);
                }
                "declined"
            }
            ("sampling/createMessage", "awaiting_user", "approve_sampling") => {
                if input.content.is_some() {
                    return Err(Error::Forbidden);
                }
                sampling_messages(
                    &r.view.original_request["params"],
                    input.edited_messages.as_deref(),
                )?;
                "approved"
            }
            ("sampling/createMessage", "awaiting_user" | "awaiting_release", "decline_sample") => {
                if input.content.is_some() || input.edited_messages.is_some() {
                    return Err(Error::Forbidden);
                }
                "declined"
            }
            ("sampling/createMessage", "awaiting_release", "release_sample") => {
                if input.content.is_some() || input.edited_messages.is_some() {
                    return Err(Error::Forbidden);
                }
                "approved"
            }
            _ => return Err(Error::Conflict),
        };
        let auth = keys
            .key_for_purpose(&key, KeyPurpose::RequestFingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let digest = fingerprint(keys, auth, &json!([run.to_string(), id, input, actor]))?;
        let aad = interaction_scope(&r.task, id, "task_interaction_decision");
        let envelope = json!({"answer":input,"actor":actor,"request_sha256":r.view.request_sha256,"operation_id":r.operation_id});
        let receipt = json!({"interaction_id":id,"task_record_id":r.task.view.id,"revision":r.view.revision+1,"state":state,"decision":input.action,"request_sha256":r.view.request_sha256,"effects_unknown":true});
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state=$1,decision_scope=$2,sealed_decision=$3,decision_actor=$4,updated_at=$5 WHERE id=$6 AND revision=$7 AND state=$8").bind(state).bind(json_string(&aad)?).bind(seal(keys,key,&aad,&envelope)?).bind(json_string(&actor)?).bind(now()).bind(id).bind(input.expected_revision).bind(&r.view.state).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        let original_aad = interaction_scope(
            &r.task,
            &format!(
                "decision:{}",
                canonical_hash(&json!([id, input.idempotency_key]))
            ),
            "task_interaction_decision",
        );
        sqlx::query("INSERT INTO mcp_task_interaction_decisions(interaction_id,request_key,expected_revision,request_sha256,request_key_id,secret_scope,sealed_decision,original_actor,receipt,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(id).bind(&input.idempotency_key).bind(input.expected_revision).bind(digest).bind(json_string(&auth)?).bind(json_string(&original_aad)?).bind(seal(keys,key,&original_aad,&envelope)?).bind(json_string(&actor)?).bind(json_string(&receipt)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    /// Atomically charge the existing eight-model-request allowance before IO.
    pub async fn admit_offline_mcp_sampling(
        &self,
        p: &OfflineMcpCallPermit,
        permit: &McpTaskOperationPermit,
        id: &str,
        intent: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        original(&mut tx, p, keys).await?;
        let mut run = current(&mut tx, &p.source, true).await?;
        let r = record(&mut tx, id, keys).await?;
        check_operation(&mut tx, &r.task, permit).await?;
        decision_current(self, &mut tx, &r).await?;
        if r.operation_id != permit.id
            || r.authority_sha256 != p.source_sha256
            || r.view.state != "approved"
            || r.view.method != "sampling/createMessage"
            || r.decision
                .as_ref()
                .is_none_or(|d| d["action"] != "approve_sampling")
            || r.view.expires_at <= now()
            || intent["source_sha256"] != p.source_sha256
            || intent["endpoint"] != p.source.ticket.grant.offline_base_url
            || intent["model"] != p.source.ticket.grant.model
            || run.model_requests >= p.source.ticket.grant.maximum_model_requests
        {
            return Err(Error::Forbidden);
        }
        let old = run.revision;
        run.model_requests += 1;
        run.revision += 1;
        let model_requests = run.model_requests;
        crate::offline_execution::append_offline_journal(
            &mut run,
            "mcp_sampling_dispatched",
            json!({"interaction_id":id,"task_record_id":r.task.view.id,"source_sha256":p.source_sha256,"request_sha256":canonical_hash(intent),"model_request":model_requests,"automatic_replay":false}),
            now(),
        )?;
        let aad = interaction_scope(&r.task, id, "task_interaction_model_intent");
        if sqlx::query("UPDATE mcp_task_interactions SET revision=revision+1,state='sampling_admitted',model_attempt_id=$1,model_intent_scope=$2,sealed_model_intent=$3,model_request_sha256=$4,updated_at=$5 WHERE id=$6 AND revision=$7 AND model_attempt_id IS NULL").bind(format!("mcp-sampling:{id}")).bind(json_string(&aad)?).bind(seal(keys,key,&aad,intent)?).bind(canonical_hash(intent)).bind(now()).bind(id).bind(r.view.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect)}
        if sqlx::query("UPDATE offline_local_runs SET payload=$1,revision=$2 WHERE id=$3 AND revision=$4 AND state='running'").bind(json_string(&run)?).bind(run.revision).bind(run.id.to_string()).bind(old).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)
    }
}
