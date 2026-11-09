//! Private sampling child claims preserve the original pending MCP checkpoint.
use super::*;
use crate::mcp_tasks::McpTaskOperationPermit;
use cowork_secrets::{McpSecretScope,SealedMcpBlob,open_mcp_blob};
use zeroize::Zeroizing;
pub(crate) async fn charge(tx:&mut Transaction<'_,Any>,run:Uuid,node_scope:Option<&str>)->Result<(u64,u64)>{
    let row=if let Some(scope)=node_scope{sqlx::query("SELECT COUNT(*) AS steps,COALESCE(SUM(charged_tokens),0) AS tokens FROM mcp_task_sampling_claims WHERE run_id=$1 AND node_scope_sha256=$2").bind(run.to_string()).bind(scope).fetch_one(&mut **tx).await.map_err(storage_error)?}else{sqlx::query("SELECT COUNT(*) AS steps,COALESCE(SUM(charged_tokens),0) AS tokens FROM mcp_task_sampling_claims WHERE run_id=$1").bind(run.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?};
    Ok((u64::try_from(row.try_get::<i64,_>("steps").map_err(storage_error)?).map_err(|_|Error::BudgetExceeded)?,u64::try_from(row.try_get::<i64,_>("tokens").map_err(storage_error)?).map_err(|_|Error::BudgetExceeded)?))
}
impl Store{
    pub async fn android_private_mcp_sampling_admit(&self,run:Uuid,lease:Uuid,sequence:i64,permit:&McpTaskOperationPermit,interaction:&str,intent:&Value,keys:&dyn KeyStore,key:KeyId)->Result<Option<Zeroizing<String>>>{
        let mut tx=self.pool.begin().await.map_err(storage_error)?;let f=tasks::facts(self,&mut tx,run,lease,sequence,keys).await?;crate::mcp_scoped_tasks::validate(&mut tx,&f,permit,key).await?;
        let current=device_writer(&mut tx).await?;let r=leased(&mut tx,run,lease,current).await?;let cp=tools::checkpoint(&mut tx,&r,keys).await?;tools::loop_authority(&mut tx,&r,&cp,current).await?;
        let profile=crate::android_private_server::authority_configuration(&mut tx,&r,current).await?;
        let parent:RunSpec=serde_json::from_value(cp["spec"].clone()).map_err(|_|device_invalid())?;let sample:RunSpec=serde_json::from_value(intent["spec"].clone()).map_err(|_|device_invalid())?;
        let node=if cp["workflow_node"].is_null(){String::new()}else{canonical_hash(&cp["workflow_node"])};
        if intent["parent_checkpoint_sha256"]!=canonical_hash(&cp)||intent["provider_sha256"]!=canonical_hash(&profile)||intent["workflow_node"]!=cp["workflow_node"]||intent["task_operation_id"]!=permit.id||sample.provider.as_ref().map(canonical_hash)!=parent.provider.as_ref().map(canonical_hash)||sample.model_binding!=parent.model_binding||sample.budgets.max_output_tokens>parent.budgets.max_output_tokens||sample.budgets.max_total_tokens!=parent.budgets.max_total_tokens||!sample.allowed_tools.is_empty()||!sample.mcp_tools.is_empty()||sample.output.is_some()||sample.project_id!=parent.project_id||sample.order_id!=parent.order_id{return Err(Error::Forbidden);}
        let reserve=u64::from(parent.provider.as_ref().ok_or(Error::Forbidden)?.context_tokens);
        let(charged_steps,charged_tokens)=charge(&mut tx,run,Some(&node)).await?;
        let steps=cp["model_steps"].as_u64().ok_or_else(device_invalid)?.checked_add(cp["tool_steps"].as_u64().ok_or_else(device_invalid)?).and_then(|n|n.checked_add(charged_steps)).and_then(|n|n.checked_add(2)).ok_or(Error::BudgetExceeded)?;
        if steps>u64::from(parent.budgets.max_steps)||cp["total_tokens"].as_u64().ok_or_else(device_invalid)?.checked_add(charged_tokens).and_then(|n|n.checked_add(reserve)).is_none_or(|n|n>parent.budgets.max_total_tokens){return Err(Error::BudgetExceeded);}
        let original=sqlx::query("SELECT model_intent_scope,sealed_model_intent,model_request_sha256 FROM mcp_task_interactions WHERE id=$1 AND operation_id=$2 AND state='sampling_admitted'").bind(interaction).bind(&permit.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let s:McpSecretScope=parse(&original.try_get::<String,_>("model_intent_scope").map_err(storage_error)?)?;let sealed:SealedMcpBlob=parse(&original.try_get::<String,_>("sealed_model_intent").map_err(storage_error)?)?;let original_intent:Value=serde_json::from_slice(&open_mcp_blob(keys,&s,&sealed).map_err(|_|Error::NativeKeyUnavailable)?).map_err(storage_error)?;
        if s.owner_kind!="personal"||s.owner_id!=f.owner_id||s.project_id!=f.project_id||s.server_id!=f.server_id||s.record_id!=interaction||s.purpose!="task_interaction_model_intent"||&original_intent!=intent||canonical_hash(intent)!=original.try_get::<String,_>("model_request_sha256").map_err(storage_error)?{return Err(Error::Forbidden);}
        let secret=tools::credential(&mut tx,&r,&profile,keys).await?;
        sqlx::query("INSERT INTO mcp_task_sampling_claims(interaction_id,run_id,node_scope_sha256,original_checkpoint_sha256,reserved_tokens,charged_tokens,state,original_request_sha256,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$5,'admitted',$6,$7,$7)").bind(interaction).bind(run.to_string()).bind(&node).bind(canonical_hash(&cp)).bind(i64::try_from(reserve).map_err(|_|Error::BudgetExceeded)?).bind(canonical_hash(intent)).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        tools::live_after_crypto(&mut tx,&r,&cp).await?;tx.commit().await.map_err(storage_error)?;Ok(secret)
    }
    pub async fn android_private_mcp_sampling_validate(&self,run:Uuid,lease:Uuid,sequence:i64,permit:&McpTaskOperationPermit,interaction:&str,intent:&Value,keys:&dyn KeyStore,key:KeyId)->Result<()> {
        let mut tx=self.pool.begin().await.map_err(storage_error)?;let f=tasks::facts(self,&mut tx,run,lease,sequence,keys).await?;crate::mcp_scoped_tasks::validate(&mut tx,&f,permit,key).await?;
        let row=sqlx::query("SELECT state,original_checkpoint_sha256,original_request_sha256 FROM mcp_task_sampling_claims WHERE interaction_id=$1 AND run_id=$2").bind(interaction).bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if row.try_get::<String,_>("state").map_err(storage_error)?!="admitted"||row.try_get::<String,_>("original_checkpoint_sha256").map_err(storage_error)?!=intent["parent_checkpoint_sha256"].as_str().ok_or(Error::Forbidden)?||row.try_get::<String,_>("original_request_sha256").map_err(storage_error)?!=canonical_hash(intent){return Err(Error::UncertainEffect);}tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_mcp_sampling_received(&self,run:Uuid,interaction:&str,response:&Value)->Result<()> {
        let mut tx=self.pool.begin().await.map_err(storage_error)?;device_writer(&mut tx).await?;
        let row=sqlx::query("SELECT c.reserved_tokens,c.original_request_sha256,i.model_response_sha256 FROM mcp_task_sampling_claims c JOIN mcp_task_interactions i ON i.id=c.interaction_id WHERE c.interaction_id=$1 AND c.run_id=$2 AND c.state='admitted' AND i.state='awaiting_release'").bind(interaction).bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if row.try_get::<String,_>("model_response_sha256").map_err(storage_error)?!=canonical_hash(response)||response["intent_sha256"]!=row.try_get::<String,_>("original_request_sha256").map_err(storage_error)?{return Err(Error::Forbidden);}
        let reserved=u64::try_from(row.try_get::<i64,_>("reserved_tokens").map_err(storage_error)?).map_err(|_|Error::BudgetExceeded)?;let measured=response["usage"]["input_tokens"].as_u64().zip(response["usage"]["output_tokens"].as_u64()).and_then(|(i,o)|i.checked_add(o));let charged=if response["usage"]["incomplete"]==true{reserved.max(measured.unwrap_or(0))}else{measured.unwrap_or(reserved)};
        if sqlx::query("UPDATE mcp_task_sampling_claims SET state='received',charged_tokens=$1,original_response_sha256=$2,updated_at=$3 WHERE interaction_id=$4 AND run_id=$5 AND state='admitted'").bind(i64::try_from(charged.max(1)).map_err(|_|Error::BudgetExceeded)?).bind(canonical_hash(response)).bind(now()).bind(interaction).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}tx.commit().await.map_err(storage_error)
    }
}
