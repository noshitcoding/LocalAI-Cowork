//! Role context comes from the actual original Team model/tool loop only.
use super::*;
use crate::mcp_interactions::{McpInteractionExecution,McpInteractionRecord,ScopedTaskInteractions,sampling_messages};
use futures_util::{future::BoxFuture,StreamExt};
#[derive(Clone)]
struct Source {provider:TeamProvider,messages:Vec<Value>}
tokio::task_local!{static ORIGINAL_SOURCE:Source;}
pub(in crate::team_service) async fn with_mcp_sampling_context<F,T>(provider:TeamProvider,messages:Vec<Value>,future:F)->T where F:std::future::Future<Output=T>{ORIGINAL_SOURCE.scope(Source{provider,messages},future).await}
pub(super) struct TeamTaskInteractions<'a>{pub authority:&'a TeamExecutionAuthority,pub context:&'a Context,pub permit:&'a McpTaskOperationPermit}
impl McpInteractionExecution for TeamTaskInteractions<'_>{
    fn validate(&self)->BoxFuture<'_,Result<()>>{Box::pin(self.authority.validate_team_task(self.context,self.permit))}
    fn response_bearer(&self)->BoxFuture<'_,Result<Option<Zeroizing<String>>>>{Box::pin(async move{self.validate().await?;let result=self.authority.team_task_bearer(self.context).await?;self.validate().await?;Ok(result)})}
    fn sample<'a>(&'a self,r:&'a McpInteractionRecord)->BoxFuture<'a,Result<()>>{Box::pin(async move{
        self.validate().await?;
        let source=ORIGINAL_SOURCE.try_with(Clone::clone).map_err(|_|Error::Forbidden)?;
        let mut tx=self.authority.store.pool.begin().await.map_err(storage_error)?;
        self.authority.task_facts(&mut tx,self.context).await?;
        let row=sqlx::query("SELECT * FROM team_provider WHERE team_id=$1 AND revision=$2").bind(self.authority.team_id().to_string()).bind(self.authority.binding.provider_revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let current=provider(&row)?;if canonical_hash(&current)!=canonical_hash(&source.provider){return Err(Error::Forbidden);}
        let slot:String=sqlx::query_scalar("SELECT slot FROM team_effect_intents WHERE run_id=$1 AND request_id=$2 AND kind='tool' AND state='admitted'").bind(self.authority.run_id().to_string()).bind(self.authority.effect.as_ref().ok_or(Error::Forbidden)?.request_id()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let step=slot.rsplit_once("/tool/").ok_or(Error::Forbidden)?.0.to_string();
        self.authority.mcp_sampling_steps(&mut tx,&step).await?;
        let sealed:SealedCredential=parse(&row.try_get::<String,_>("sealed_key").map_err(storage_error)?)?;if sealed.key_id()!=self.authority.key_id{return Err(Error::NativeKeyUnavailable);}
        let credential=open_bearer(self.authority.keys.as_ref(),&super::super::scope(&current),&sealed).map_err(|_|Error::NativeKeyUnavailable)?;
        tx.commit().await.map_err(storage_error)?;
        let mut messages=source.messages.iter().filter(|m|m.get("tool_calls").is_none()&&m["role"]!="tool").cloned().collect::<Vec<_>>();
        let decision=r.decision.as_ref().ok_or(Error::Forbidden)?;
        messages.extend(sampling_messages(&r.view.original_request["params"],decision["edited_messages"].as_array().map(Vec::as_slice))?);
        let url=format!("{}/chat/completions",current.base_url);
        let body=json!({"model":current.model,"messages":messages,"stream":false,"max_tokens":r.view.original_request["params"]["maxTokens"].as_u64().ok_or(Error::Forbidden)?.min(65536)});
        if json_string(&body)?.len()>2*1024*1024{return Err(Error::ContextLimit);}
        let request_id=format!("mcp-sampling:{}",r.view.id);
        let intent=json!({"format":1,"interaction_id":r.view.id,"task_operation_id":self.permit.id,"parent_effect_request_id":self.authority.effect.as_ref().ok_or(Error::Forbidden)?.request_id(),"provider":current,"body":body,"endpoint":url,"request_id":request_id,"original_role_context_sha256":canonical_hash(&source.messages)});
        self.authority.store.admit_mcp_interaction_sampling(self.permit,&r.view.id,&intent,self.authority.keys.as_ref(),self.authority.key_id).await?;
        self.validate().await?;
        self.authority.store.mcp_task_interaction(self.permit,&r.view.id,self.authority.keys.as_ref()).await?;
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never()).pool_max_idle_per_host(0).timeout(Duration::from_secs(180)).build().map_err(|_|Error::UncertainEffect)?;
        let response=client.post(&url).bearer_auth(credential.as_str()).header("x-cowork-request-id",&request_id).header("idempotency-key",&request_id).json(&body).send().await.map_err(|_|Error::UncertainEffect)?;
        let status=response.status().as_u16();let mut bytes=Vec::new();let mut stream=response.bytes_stream();
        while let Some(chunk)=stream.next().await{let chunk=chunk.map_err(|_|Error::UncertainEffect)?;if bytes.len()+chunk.len()>2*1024*1024{return Err(Error::BudgetExceeded);}bytes.extend_from_slice(&chunk);}
        let value:Value=serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let content=value.pointer("/choices/0/message/content").and_then(Value::as_str).filter(|s|s.len()<=1024*1024);
        let valid=(200..300).contains(&status)&&content.is_some()&&!value.pointer("/choices/0/message/tool_calls").is_some_and(|v|!v.as_array().is_some_and(Vec::is_empty));
        let protected=json!({"result":if valid{json!({"role":"assistant","content":{"type":"text","text":content},"model":current.model,"stopReason":"endTurn"})}else{Value::Null},"original_provider_response":value,"original_provider_bytes_base64":STANDARD.encode(&bytes),"http_status":status,"intent_sha256":canonical_hash(&intent)});
        self.authority.store.capture_mcp_interaction_sampling(self.permit,&r.view.id,&protected,self.authority.keys.as_ref(),self.authority.key_id).await?;
        if !valid{return Err(Error::UncertainEffect);}
        self.validate().await?;Ok(())
    })}
}
pub(super) fn handler<'a>(authority:&'a TeamExecutionAuthority,context:&'a Context,permit:&'a McpTaskOperationPermit,execution:&'a TeamTaskInteractions<'a>,deadline:i64)->ScopedTaskInteractions<'a>{let _=context;ScopedTaskInteractions{store:&authority.store,permit,keys:authority.keys.as_ref(),key:authority.key_id,execution,deadline}}
impl TeamExecutionAuthority{
    async fn mcp_sampling_steps(&self,tx:&mut Transaction<'_,Any>,step:&str)->Result<()> {
        validate_binding(&self.store,tx,&self.binding,&self.lease_owner).await?;let prefix=format!("{step}/model/");let toolprefix=format!("{step}/tool/");
        let models:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_effect_intents WHERE run_id=$1 AND kind='model' AND SUBSTR(slot,1,LENGTH($2))=$2").bind(self.run_id().to_string()).bind(&prefix).fetch_one(&mut **tx).await.map_err(storage_error)?;
        let children:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id JOIN team_effect_intents e ON e.run_id=t.run_id AND e.request_id=t.tool_call_id WHERE t.owner_kind='team' AND t.owner_id=$1 AND t.run_id=$2 AND SUBSTR(e.slot,1,LENGTH($3))=$3 AND i.model_attempt_id IS NOT NULL").bind(self.team_id().to_string()).bind(self.run_id().to_string()).bind(&toolprefix).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if models.checked_add(children).is_none_or(|n|n>=8){return Err(Error::BudgetExceeded);}Ok(())
    }
    pub(in crate::team_service) async fn assert_mcp_sampling_steps(&self,step:&str)->Result<()> {let mut tx=self.store.pool.begin().await.map_err(storage_error)?;self.mcp_sampling_steps(&mut tx,step).await?;tx.commit().await.map_err(storage_error)}
}
