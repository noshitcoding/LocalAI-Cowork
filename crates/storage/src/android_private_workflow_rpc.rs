//! Original Crew guest callbacks. Existing dispatches are evidence, never resend permits.
use super::execution::{
    current_scope, put_version, save_node_context, scoped_authority, scoped_checkpoint,
};
use super::*;
use crate::android_private_server::{id, leased, row};
use crate::android_private_workspace as workspace;
use cowork_contracts::{RunSpec, ToolCall};

pub enum PrivateCrewRpc {
    Dispatch(PrivateCrewRpcPermit),
    Received(Value),
}
pub struct PrivateCrewRpcPermit {
    run: Uuid,
    lease: Uuid,
    node: String,
    source: Uuid,
    nonce: Uuid,
    kind: String,
    request_sha: String,
    original: Value,
}
impl PrivateCrewRpcPermit {
    pub fn source(&self) -> Uuid {
        self.source
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha
    }
    pub fn original(&self) -> &Value {
        &self.original
    }
}
fn checked(raw: &[u8], cp: &Value) -> Result<(Uuid, String, Value)> {
    if raw.is_empty() || raw.len() > 4 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let v: Value = serde_json::from_slice(raw).map_err(storage_error)?;
    let source = crate::android_personal::wire_uuid(&v, "source")?;
    let kind = v["kind"]
        .as_str()
        .filter(|v| matches!(*v, "model" | "tool" | "result"))
        .ok_or_else(device_invalid)?
        .to_owned();
    if v["format"] != 1
        || v["node_id"] != cp["workflow_node"]["node_id"]
        || v["role_id"] != cp["workflow_node"]["role_id"]
        || v["original_plan_sha256"] != cp["workflow_node"]["plan_sha256"]
        || v.as_object().is_none_or(|m| m.len() != 7)
    {
        return Err(Error::Forbidden);
    }
    Ok((source, kind, v))
}
async fn physical(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    nonce: Uuid,
    lease: Uuid,
) -> Result<()> {
    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND project_id=$4 AND lease_id=$5 AND state='running' AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(id(r,"id")?.to_string()).bind(id(r,"owner_id")?.to_string()).bind(id(r,"project_id")?.to_string()).bind(lease.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if n != 1 {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    /// Consumes the only callback permit before any native admission. This is
    /// an original negative acknowledgement, never a fabricated model/tool answer.
    pub async fn android_private_crew_rpc_no_dispatch(
        &self,
        permit: PrivateCrewRpcPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let key = workspace::writer_cipher(keys, key)?;
        let scope = current_scope().ok_or(Error::Forbidden)?;
        if scope.run_id() != permit.run
            || scope.node_id() != permit.node
            || scope.lease() != permit.lease
            || permit.kind == "result"
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, permit.run).await?;
        let rpc=sqlx::query("SELECT * FROM android_private_workflow_rpcs WHERE run_id=$1 AND source_id=$2 AND node_id=$3 AND original_lease_id=$4 AND container_nonce=$5 AND request_sha256=$6 AND state='admitted' AND reply_version_id IS NULL").bind(permit.run.to_string()).bind(permit.source.to_string()).bind(&permit.node).bind(permit.lease.to_string()).bind(permit.nonce.to_string()).bind(&permit.request_sha).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let cp = scoped_checkpoint(&mut tx, &r, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        if canonical_hash(&cp)
            != rpc
                .try_get::<String, _>("context_sha256")
                .map_err(storage_error)?
            || cp["crew_rpc_source"] != permit.source.to_string()
        {
            return Err(Error::UncertainEffect);
        }
        let invocation = if permit.kind == "model" {
            format!(
                "workflow:{}:{}:model:{}",
                hash(scope.node_id().as_bytes()),
                scope.claim(),
                cp["model_steps"]
                    .as_u64()
                    .ok_or(Error::Forbidden)?
                    .checked_add(1)
                    .ok_or(Error::BudgetExceeded)?
            )
        } else {
            format!(
                "workflow:{}:{}:tool:{}:{}",
                hash(scope.node_id().as_bytes()),
                scope.claim(),
                cp["model_steps"].as_u64().ok_or(Error::Forbidden)?,
                permit.source
            )
        };
        let admitted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2",
        )
        .bind(permit.run.to_string())
        .bind(&invocation)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if admitted != 0 {
            return Err(Error::UncertainEffect);
        }
        let reply = json!({"format":"android-private-crew-response/1","run_id":permit.run,"node_id":permit.node,"container_nonce":permit.nonce,"source":permit.source,"request_sha256":permit.request_sha,"ok":false,"result":{"never_native_admitted":true,"reason":"cancelled_before_native_admission"}});
        let vid = put_version(&mut tx, &r, "crew-response", &reply, keys, key, now).await?;
        if sqlx::query("UPDATE android_private_workflow_rpcs SET state='received',reply_version_id=$1,reply_sha256=$2 WHERE run_id=$3 AND source_id=$4 AND state='admitted' AND reply_version_id IS NULL AND context_sha256=$5").bind(vid).bind(canonical_hash(&reply)).bind(permit.run.to_string()).bind(permit.source.to_string()).bind(canonical_hash(&cp)).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        let mut after = cp.clone();
        after["pending_tools"]
            .as_array_mut()
            .ok_or(Error::Forbidden)?
            .clear();
        after["last_turn_has_tools"] = json!(false);
        after["crew_original_no_dispatch"] =
            json!({"source":permit.source,"receipt_sha256":canonical_hash(&reply)});
        save_node_context(&mut tx, &r, &scope, &after, &cp, keys, key, now).await?;
        sqlx::query("UPDATE android_private_loop_requests SET state='expired' WHERE run_id=$1 AND invocation_id=$2 AND state='pending'").bind(permit.run.to_string()).bind(invocation).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_crew_rpc(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        raw: &[u8],
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<PrivateCrewRpc> {
        let key = workspace::writer_cipher(keys, key)?;
        let scope = current_scope().ok_or(Error::Forbidden)?;
        if scope.run_id() != run || scope.lease() != lease {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let original_run: Value =
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if original_run["spec"]["input"]["private_workflow"]["engine"] != "crew" {
            return Err(Error::Forbidden);
        }
        let cp = scoped_checkpoint(&mut tx, &r, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        scoped_authority(&mut tx, &r, &cp, now).await?;
        physical(&mut tx, &r, nonce, lease).await?;
        let (source, kind, request) = checked(raw, &cp)?;
        let request_sha = hash(raw);
        if let Some(old) = sqlx::query(
            "SELECT * FROM android_private_workflow_rpcs WHERE run_id=$1 AND source_id=$2",
        )
        .bind(run.to_string())
        .bind(source.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if old.try_get::<String, _>("node_id").map_err(storage_error)? != scope.node_id()
                || old
                    .try_get::<String, _>("original_lease_id")
                    .map_err(storage_error)?
                    != lease.to_string()
                || old
                    .try_get::<String, _>("container_nonce")
                    .map_err(storage_error)?
                    != nonce.to_string()
                || old.try_get::<String, _>("kind").map_err(storage_error)? != kind
                || old
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != request_sha
            {
                return Err(Error::Conflict);
            }
            let vid: String = old.try_get("request_version_id").map_err(storage_error)?;
            let bytes = workspace::version_bytes(&mut tx, &r, &vid, keys).await?;
            let retained: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
            if retained["request_sha256"] != request_sha
                || retained["original_base64"]
                    != base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
            {
                return Err(Error::Forbidden);
            }
            if old.try_get::<String, _>("state").map_err(storage_error)? != "received" {
                return Err(Error::UncertainEffect);
            }
            let reply = execution::version(
                &mut tx,
                &r,
                &old.try_get::<String, _>("reply_version_id")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            if Some(canonical_hash(&reply))
                != old
                    .try_get::<Option<String>, _>("reply_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            crate::android_private_tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(PrivateCrewRpc::Received(reply));
        }
        let outstanding:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_workflow_rpcs WHERE run_id=$1 AND node_id=$2 AND state!='received'").bind(run.to_string()).bind(scope.node_id()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if outstanding != 0 || !cp["pending_tools"].as_array().is_some_and(Vec::is_empty) {
            return Err(Error::UncertainEffect);
        }
        let mut next = cp.clone();
        let mut spec: RunSpec =
            serde_json::from_value(cp["workflow_node"]["authorized_spec"].clone())
                .map_err(storage_error)?;
        next["crew_rpc_source"] = json!(source);
        next["crew_rpc_kind"] = json!(kind);
        match kind.as_str() {
            "model" => {
                let messages = request["body"]["messages"]
                    .as_array()
                    .filter(|m| !m.is_empty() && m.len() <= 512)
                    .ok_or_else(device_invalid)?;
                if json_string(messages)?.len() > 2 * 1024 * 1024 {
                    return Err(Error::ContextLimit);
                }
                if messages.iter().any(|m| {
                    m["content"].as_str().is_none()
                        || !matches!(m["role"].as_str(), Some("system" | "assistant" | "user"))
                }) {
                    return Err(device_invalid());
                }
                spec.allowed_tools.clear();
                spec.mcp_tools.clear();
                let system = cp["messages"]
                    .as_array()
                    .and_then(|a| a.first())
                    .cloned()
                    .ok_or(Error::Forbidden)?;
                next["messages"] = json!([system,{"role":"user","content":format!("Original Crew reasoning context (data): {}",json_string(messages)?)}]);
                if cp["messages"].as_array().is_some_and(|messages| {
                    messages.iter().any(|m| {
                        m["role"] == "system"
                            && m["cowork_phase"] == cowork_domain::HYBRID_EXECUTION_MARKER
                    })
                }) {
                    next["messages"].as_array_mut().ok_or(Error::Forbidden)?.push(json!({"role":"system","cowork_phase":cowork_domain::HYBRID_EXECUTION_MARKER,"content":"Continue execution of the reviewed plan under the existing permissions and budgets."}));
                }
                next["last_content"] = Value::Null;
                next["last_turn_has_tools"] = json!(false);
            }
            "tool" => {
                if spec.agent_config.as_ref().and_then(|c| c.execution_mode)
                    == Some(AgentExecutionMode::Plan)
                {
                    return Err(Error::Forbidden);
                }
                if cowork_domain::hybrid_planning(
                    &spec,
                    cp["messages"].as_array().ok_or(Error::Forbidden)?,
                ) {
                    return Err(Error::Forbidden);
                }
                let name = request["body"]["name"]
                    .as_str()
                    .filter(|n| spec.allowed_tools.iter().any(|t| t == *n))
                    .ok_or(Error::Forbidden)?;
                let arguments = request["body"]["arguments"]
                    .as_object()
                    .ok_or_else(device_invalid)?;
                let tool = ToolCall {
                    id: source.to_string(),
                    name: name.into(),
                    arguments: json!(arguments),
                };
                next["pending_tools"] = json!([tool]);
                next["last_turn_has_tools"] = json!(true);
            }
            "result" => {
                let content = request["body"]["content"]
                    .as_str()
                    .filter(|v| v.len() <= 1024 * 1024)
                    .ok_or_else(device_invalid)?;
                next["last_content"] = json!(content);
                next["last_turn_has_tools"] = json!(false);
            }
            _ => return Err(Error::Forbidden),
        }
        next["spec"] = serde_json::to_value(spec).map_err(storage_error)?;
        let original = json!({"format":"android-private-crew-request/1","run_id":run,"lease_id":lease,"container_nonce":nonce,"workflow_node":cp["workflow_node"],"source_id":source,"request_sha256":request_sha,"original_base64":base64::Engine::encode(&base64::engine::general_purpose::STANDARD,raw),"request":request});
        let vid = put_version(&mut tx, &r, "crew-request", &original, keys, key, now).await?;
        save_node_context(&mut tx, &r, &scope, &next, &cp, keys, key, now).await?;
        sqlx::query("INSERT INTO android_private_workflow_rpcs(run_id,node_id,source_id,original_lease_id,container_nonce,kind,request_sha256,request_version_id,context_sha256,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'admitted',$10)").bind(run.to_string()).bind(scope.node_id()).bind(source.to_string()).bind(lease.to_string()).bind(nonce.to_string()).bind(&kind).bind(&request_sha).bind(vid).bind(canonical_hash(&next)).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_tools::live_after_crypto(self, &mut tx, &r, &next).await?;
        physical(&mut tx, &r, nonce, lease).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateCrewRpc::Dispatch(PrivateCrewRpcPermit {
            run,
            lease,
            node: scope.node_id().into(),
            source,
            nonce,
            kind,
            request_sha,
            original: request,
        }))
    }
    /// Receipt admission remains possible after current runtime authority changes.
    /// It can preserve the original answer, but cannot admit another effect.
    pub async fn android_private_crew_rpc_received(
        &self,
        permit: PrivateCrewRpcPermit,
        result: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        let key = workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, permit.run).await?;
        let old=sqlx::query("SELECT * FROM android_private_workflow_rpcs WHERE run_id=$1 AND node_id=$2 AND source_id=$3 AND original_lease_id=$4 AND container_nonce=$5 AND request_sha256=$6 AND kind=$7").bind(permit.run.to_string()).bind(&permit.node).bind(permit.source.to_string()).bind(permit.lease.to_string()).bind(permit.nonce.to_string()).bind(&permit.request_sha).bind(&permit.kind).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let reply = json!({"format":"android-private-crew-response/1","run_id":permit.run,"node_id":permit.node,"container_nonce":permit.nonce,"source":permit.source,"request_sha256":permit.request_sha,"ok":true,"result":result});
        if json_string(&reply)?.len() > 3 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        if old.try_get::<String, _>("state").map_err(storage_error)? == "received" {
            let retained = execution::version(
                &mut tx,
                &r,
                &old.try_get::<String, _>("reply_version_id")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            if retained != reply
                || Some(canonical_hash(&retained))
                    != old
                        .try_get::<Option<String>, _>("reply_sha256")
                        .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(retained);
        }
        let scope = current_scope().ok_or(Error::Forbidden)?;
        if scope.run_id() != permit.run
            || scope.node_id() != permit.node
            || scope.lease() != permit.lease
        {
            return Err(Error::Forbidden);
        }
        let n=sqlx::query("SELECT * FROM android_private_workflow_nodes WHERE run_id=$1 AND node_id=$2 AND claim_token=$3 AND parent_lease_id=$4").bind(permit.run.to_string()).bind(scope.node_id()).bind(scope.claim()).bind(permit.lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let cp = execution::version(
            &mut tx,
            &r,
            &n.try_get::<String, _>("context_version_id")
                .map_err(storage_error)?,
            keys,
        )
        .await?;
        if Some(canonical_hash(&cp))
            != n.try_get::<Option<String>, _>("context_sha256")
                .map_err(storage_error)?
            || cp["crew_rpc_source"] != permit.source.to_string()
        {
            return Err(Error::Forbidden);
        }
        let mut sequence = None;
        if permit.kind == "model" {
            if cp["last_turn_has_tools"] == true
                || !cp["pending_tools"].as_array().is_some_and(Vec::is_empty)
                || result["content"] != cp["last_content"]
            {
                return Err(Error::UncertainEffect);
            }
            sequence = cp["last_model_sequence"].as_i64();
        } else if permit.kind == "tool" {
            if !cp["pending_tools"].as_array().is_some_and(Vec::is_empty) {
                return Err(Error::UncertainEffect);
            }
            let invocation = format!(
                "workflow:{}:{}:tool:{}:{}",
                hash(scope.node_id().as_bytes()),
                scope.claim(),
                cp["model_steps"].as_u64().ok_or(Error::Forbidden)?,
                permit.source
            );
            sequence=sqlx::query_scalar("SELECT sequence FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2 AND state='received'").bind(permit.run.to_string()).bind(invocation).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        } else if permit.kind == "result"
            && cp["last_content"] != permit.original["body"]["content"]
        {
            return Err(Error::Forbidden);
        }
        if permit.kind != "result" && sequence.is_none() {
            return Err(Error::UncertainEffect);
        }
        let vid = put_version(&mut tx, &r, "crew-response", &reply, keys, key, now).await?;
        if sqlx::query("UPDATE android_private_workflow_rpcs SET state='received',reply_version_id=$1,reply_sha256=$2,step_sequence=$3 WHERE run_id=$4 AND source_id=$5 AND state IN('admitted','uncertain') AND reply_version_id IS NULL AND request_sha256=$6").bind(vid).bind(canonical_hash(&reply)).bind(sequence).bind(permit.run.to_string()).bind(permit.source.to_string()).bind(permit.request_sha).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        tx.commit().await.map_err(storage_error)?;
        Ok(reply)
    }
}
