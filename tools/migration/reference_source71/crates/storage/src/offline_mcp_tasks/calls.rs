use super::source::*;
use super::*;
use crate::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord, lock_task};
use cowork_mcp::{
    CallResult, CheckpointScope, PreparedCall, PreparedTaskOperation, Session, SessionCipher,
};
use cowork_secrets::{KeyStore, McpSecretScope};
impl Store {
    pub async fn prepare_offline_mcp_call(
        &self,
        run: Uuid,
        prepared: &PreparedCall,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<OfflineMcpCallPermit> {
        if !self.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        // Load immutable source before holding the writer; current() repeats all
        // admission checks under the local guard after every key/SQL wait.
        let e = self.offline_local_execution(run).await?;
        let tool = e.run.pending_tools.first().ok_or(Error::Forbidden)?.clone();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        let cfg = configuration(&mut tx, e.ticket.grant.id).await?;
        let binding = cfg
            .mcp_connections
            .iter()
            .find(|b| Some(b.id.as_str()) == tool.arguments["connection_id"].as_str())
            .ok_or(Error::Forbidden)?;
        if prepared.endpoint()
            != cowork_mcp::Endpoint::new(&binding.endpoint, None)
                .map_err(|_| Error::Forbidden)?
                .url()
            || prepared.catalog_sha256() != binding.catalog_sha256
            || Some(prepared.tool_name()) != tool.arguments["tool_name"].as_str()
            || prepared.arguments() != &tool.arguments["arguments"]
        {
            return Err(Error::Forbidden);
        }
        // Session::call increments its epoch exactly once before the original send.
        // Bind the ACK's resulting epoch without adopting a later connection.
        let s = Source {
            format: 1,
            id: Uuid::new_v4().to_string(),
            ticket: e.ticket,
            run,
            ordinal: e.run.ordinal,
            tool,
            configuration_sha256: canonical_hash(&Some(&cfg)),
            endpoint: binding.endpoint.clone(),
            server_id: binding.id.clone(),
            catalog_sha256: binding.catalog_sha256.clone(),
            connection_id: prepared.connection_id(),
            connection_epoch: prepared.epoch().checked_add(1).ok_or(Error::Forbidden)?,
            original_rpc_id: prepared.rpc_request_id().into(),
            transport_request_sha256: prepared.request_sha256().into(),
            protocol: prepared.protocol().into(),
            task_ttl_ms: prepared.task_ttl_ms(),
            deadline: e.run.deadline_at,
        };
        current(&mut tx, &s, true).await?;
        let aad = scope(&s, "task_checkpoint");
        let value = sealed_value(&s)?;
        let sha = canonical_hash(&value);
        sqlx::query("INSERT INTO offline_mcp_calls(id,run_id,grant_id,device_id,tool_id,tool_request_sha256,configuration_sha256,connection_id,connection_epoch,original_rpc_id,transport_request_sha256,scope,sealed_source,source_sha256,state,dispatch_admitted,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'prepared',0,$15,$15)").bind(&s.id).bind(run.to_string()).bind(s.ticket.grant.id.to_string()).bind(key.instance.to_string()).bind(&s.tool.id).bind(&s.tool.request_sha256).bind(&s.configuration_sha256).bind(&s.connection_id).bind(i64::try_from(s.connection_epoch).map_err(|_|Error::Forbidden)?).bind(&s.original_rpc_id).bind(&s.transport_request_sha256).bind(json_string(&aad)?).bind(seal(keys,key,&aad,&value)?).bind(&sha).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        current(&mut tx, &s, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(OfflineMcpCallPermit {
            source: s,
            source_sha256: sha,
        })
    }
    pub async fn admit_offline_mcp_call(
        &self,
        p: &OfflineMcpCallPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, true).await?;
        if sqlx::query("UPDATE offline_mcp_calls SET state='admitted',dispatch_admitted=1,updated_at=$1 WHERE id=$2 AND state='prepared' AND dispatch_admitted=0 AND sealed_receipt IS NULL").bind(now()).bind(&p.source.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect)}
        tx.commit().await.map_err(storage_error)
    }
    pub async fn validate_offline_mcp_call(
        &self,
        p: &OfflineMcpCallPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        let row = original(&mut tx, p, keys).await?;
        if !matches!(
            row.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "admitted" | "received"
        ) || row
            .try_get::<i64, _>("dispatch_admitted")
            .map_err(storage_error)?
            != 1
        {
            return Err(Error::UncertainEffect);
        }
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn capture_offline_mcp_call(
        &self,
        p: &OfflineMcpCallPermit,
        result: &CallResult,
        session: &Session,
        cipher: &SessionCipher,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Option<McpTaskRecord>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        let row = original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, false).await?;
        if row
            .try_get::<i64, _>("dispatch_admitted")
            .map_err(storage_error)?
            != 1
            || !matches!(
                row.try_get::<String, _>("state")
                    .map_err(storage_error)?
                    .as_str(),
                "admitted" | "unknown"
            )
            || row
                .try_get::<Option<String>, _>("sealed_receipt")
                .map_err(storage_error)?
                .is_some()
            || session.connection_id() != p.source.connection_id
            || session.epoch() != p.source.connection_epoch
            || session.protocol_version() != p.source.protocol
            || canonical_hash(&result.response) != result.response_sha256
        {
            return Err(Error::UncertainEffect);
        }
        let task = if result.task.is_some() {
            let cp = CheckpointScope {
                project_id: p.source.ticket.grant.project_id.to_string(),
                run_id: p.source.run.to_string(),
                server_id: p.source.server_id.clone(),
                connection_record_id: p.source.id.clone(),
                server_revision: 1,
                checkpoint_revision: 1,
                configuration_sha256: p.source.configuration_sha256.clone(),
                catalog_sha256: p.source.catalog_sha256.clone(),
            };
            Some(
                crate::mcp_scoped_tasks::capture(
                    &mut tx,
                    &facts(p),
                    result,
                    session,
                    Some((cipher, cp)),
                    keys,
                    key,
                )
                .await?,
            )
        } else {
            None
        };
        let value = json!({"kind":"confirmed","response":result.response,"response_sha256":result.response_sha256,"catalog_changed":result.catalog_changed,"task_record_id":task.as_ref().map(|t|&t.view.id),"source_sha256":p.source_sha256});
        let aad = scope(&p.source, "task_response");
        if sqlx::query("UPDATE offline_mcp_calls SET state='received',task_record_id=$1,receipt_scope=$2,sealed_receipt=$3,receipt_sha256=$4,updated_at=$5 WHERE id=$6 AND sealed_receipt IS NULL AND dispatch_admitted=1 AND state IN('admitted','unknown')").bind(task.as_ref().map(|t|&t.view.id)).bind(json_string(&aad)?).bind(seal(keys,key,&aad,&value)?).bind(canonical_hash(&value)).bind(now()).bind(&p.source.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect)}
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
    pub async fn unknown_offline_mcp_call(&self, p: &OfflineMcpCallPermit) -> Result<()> {
        sqlx::query("UPDATE offline_mcp_calls SET state='unknown',updated_at=$1 WHERE id=$2 AND dispatch_admitted=1 AND state='admitted'").bind(now()).bind(&p.source.id).execute(&self.pool).await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn capture_offline_mcp_call_failure(
        &self,
        p: &OfflineMcpCallPermit,
        response: &Value,
        code: &str,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cipher_writer(&mut tx, key).await?;
        original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, false).await?;
        let value = json!({"kind":"unconfirmed","response":response,"response_sha256":canonical_hash(response),"code":code,"source_sha256":p.source_sha256,"effects_unknown":true});
        let aad = scope(&p.source, "task_response");
        if sqlx::query("UPDATE offline_mcp_calls SET state='unknown',receipt_scope=$1,sealed_receipt=$2,receipt_sha256=$3,updated_at=$4 WHERE id=$5 AND dispatch_admitted=1 AND state IN('admitted','unknown') AND sealed_receipt IS NULL").bind(json_string(&aad)?).bind(seal(keys,key,&aad,&value)?).bind(canonical_hash(&value)).bind(now()).bind(&p.source.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect)}
        tx.commit().await.map_err(storage_error)
    }
    pub async fn begin_offline_mcp_task_operation(
        &self,
        p: &OfflineMcpCallPermit,
        task: &McpTaskRecord,
        prepared: &PreparedTaskOperation,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskOperationPermit> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, true).await?;
        let permit =
            crate::mcp_scoped_tasks::begin(&mut tx, &facts(p), task, prepared, keys, key).await?;
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(permit)
    }
    pub async fn validate_offline_mcp_task_operation(
        &self,
        p: &OfflineMcpCallPermit,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, true).await?;
        crate::mcp_scoped_tasks::validate(&mut tx, &facts(p), permit, key).await?;
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn commit_offline_mcp_task_operation(
        &self,
        p: &OfflineMcpCallPermit,
        permit: &McpTaskOperationPermit,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<McpTaskRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        original(&mut tx, p, keys).await?;
        current(&mut tx, &p.source, true).await?;
        let task = crate::mcp_scoped_tasks::commit(&mut tx, &facts(p), permit, keys, key).await?;
        current(&mut tx, &p.source, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
    pub async fn offline_mcp_original_result(
        &self,
        run: Uuid,
        tool: &OfflinePendingTool,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let row=sqlx::query("SELECT c.*,g.ticket AS original_ticket FROM offline_mcp_calls c JOIN offline_local_grants g ON g.id=c.grant_id WHERE c.run_id=$1 AND c.tool_id=$2 AND c.tool_request_sha256=$3").bind(run.to_string()).bind(&tool.id).bind(&tool.request_sha256).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let p = row_source(&row, keys)?;
        current(&mut tx, &p.source, false).await?;
        let aad: McpSecretScope = parse(
            &row.try_get::<String, _>("receipt_scope")
                .map_err(storage_error)?,
        )?;
        if aad != scope(&p.source, "task_response") {
            return Err(Error::Forbidden);
        }
        let receipt = open(
            keys,
            &aad,
            &row.try_get::<String, _>("sealed_receipt")
                .map_err(storage_error)?,
        )?;
        if canonical_hash(&receipt)
            != row
                .try_get::<String, _>("receipt_sha256")
                .map_err(storage_error)?
            || receipt["source_sha256"] != p.source_sha256
            || canonical_hash(&receipt["response"]) != receipt["response_sha256"]
        {
            return Err(Error::Forbidden);
        }
        if receipt["kind"] != "confirmed" {
            return Err(Error::UncertainEffect);
        }
        let response = if let Some(id) = row
            .try_get::<Option<String>, _>("task_record_id")
            .map_err(storage_error)?
        {
            let task = lock_task(&mut tx, &id).await?;
            crate::mcp_scoped_tasks::check(&facts(&p), &task)?;
            let secret = task.open(keys)?;
            let row=sqlx::query("SELECT final_result_scope,sealed_final_result,final_result_sha256 FROM mcp_async_tasks WHERE id=$1").bind(&id).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let aad: McpSecretScope = parse(
                &row.try_get::<String, _>("final_result_scope")
                    .map_err(storage_error)?,
            )?;
            let blob: String = row.try_get("sealed_final_result").map_err(storage_error)?;
            let result = open(keys, &aad, &blob)?;
            if aad.owner_kind!="offline"||aad.owner_id!=task.owner_id||aad.project_id!=task.project_id||aad.server_id!=task.view.server_id||aad.configuration_sha256!=task.binding_sha256||aad.purpose!="task_response"||sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM mcp_async_task_operations WHERE id=$1 AND task_record_id=$2 AND operation='result' AND state='committed' AND secret_scope=$3 AND sealed_response=$4").bind(&aad.record_id).bind(&id).bind(json_string(&aad)?).bind(&blob).fetch_one(&mut *tx).await.map_err(storage_error)?!=1||canonical_hash(&result["response"])!=row.try_get::<String,_>("final_result_sha256").map_err(storage_error)?{return Err(Error::Forbidden)}
            crate::mcp_tasks::result_state(&secret.current, &result)?;
            result["response"].clone()
        } else {
            receipt["response"].clone()
        };
        let value = json!({"connection_id":p.source.server_id,"tool_name":p.source.tool.arguments["tool_name"],"response_sha256":canonical_hash(&response),"response":response,"catalog_changed":receipt["catalog_changed"]});
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
