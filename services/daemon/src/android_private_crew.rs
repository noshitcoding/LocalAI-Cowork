//! Installed CrewAI runs only in the pinned private gVisor image, behind Actor-owned RPC.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_storage::android_private_workflows::PrivateCrewRpc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    image: String,
    crewai_version: String,
    crewai_source_sha256: String,
    bridge_sha256: String,
}
pub(super) struct PrivateCrewRuntime {
    configuration: Value,
}
impl PrivateCrewRuntime {
    pub(super) fn new(instance: Uuid, runner: &Value, value: Value) -> Result<Self> {
        let c: Configuration = serde_json::from_value(value).map_err(|_| invalid())?;
        if c.format != 1
            || c.image != runner["image"].as_str().ok_or_else(invalid)?
            || !c.image.contains("@sha256:")
            || c.crewai_version.is_empty()
            || c.crewai_version.len() > 100
            || c.crewai_source_sha256.len() != 64
            || !c
                .crewai_source_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || c.bridge_sha256
                != cowork_domain::hash(include_bytes!("../../../deploy/v2/android-private-crew.py"))
        {
            return Err(Error::Forbidden);
        }
        let configuration = json!({"format":"android-private-workflow-engine/1","engine":"crew","instance_id":instance,"host_id":runner["host_id"],"engine_id":runner["engine_id"],"runner_binding_sha256":canonical_hash(runner),"image":c.image,"crewai_version":c.crewai_version,"crewai_source_sha256":c.crewai_source_sha256,"bridge_sha256":c.bridge_sha256,"dispatcher_sha256":cowork_domain::hash(include_bytes!("android_private_workflow_worker.rs"))});
        Ok(Self { configuration })
    }
    pub(super) fn configuration(&self) -> &Value {
        &self.configuration
    }
}

async fn frame(read: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> Result<Value> {
    let mut raw = Vec::new();
    loop {
        let available = read.fill_buf().await.map_err(|_| Error::UncertainEffect)?;
        if available.is_empty() {
            return Err(Error::UncertainEffect);
        }
        let end = available.iter().position(|b| *b == b'\n');
        let take = end.map_or(available.len(), |i| i + 1);
        if raw
            .len()
            .checked_add(take)
            .is_none_or(|n| n > 6 * 1024 * 1024)
        {
            return Err(Error::BudgetExceeded);
        }
        raw.extend_from_slice(&available[..take]);
        read.consume(take);
        if end.is_some() {
            return serde_json::from_slice(&raw).map_err(|_| invalid());
        }
    }
}
async fn write_frame(write: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let raw = Zeroizing::new(serde_json::to_vec(value).map_err(|_| invalid())?);
    if raw.len() > 4 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    write
        .write_all(&raw)
        .await
        .map_err(|_| Error::UncertainEffect)?;
    write
        .write_all(b"\n")
        .await
        .map_err(|_| Error::UncertainEffect)?;
    write.flush().await.map_err(|_| Error::UncertainEffect)
}
impl AndroidDeviceServices {
    pub(super) async fn private_crew_node(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        scope: &PrivateWorkflowNodeScope,
        stop: &CancellationToken,
        shared: Arc<Mutex<Tools>>,
    ) -> Result<()> {
        let runtime = self
            .private_workflows
            .as_ref()
            .and_then(|s| s.crew.as_ref())
            .ok_or(Error::Forbidden)?;
        let frozen = &lease.value["spec"]["input"];
        if frozen["private_workflow"]["engine_stamp"]["config_sha256"]
            != canonical_hash(runtime.configuration())
        {
            return Err(Error::Forbidden);
        }
        let runner = self.private_runner.as_ref().ok_or(Error::Forbidden)?;
        if frozen["frozen_private_runner"] != runner.binding() {
            return Err(Error::Forbidden);
        }
        let cp = self
            .store
            .android_private_loop_checkpoint(lease.run, lease.lease, self.keys.as_ref())
            .await?;
        let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).map_err(|_| invalid())?;
        let graph = self
            .store
            .android_private_workflow_checkpoint(lease.run, lease.lease, self.keys.as_ref())
            .await?;
        let plan: WorkflowPlan =
            serde_json::from_value(graph["workflow"]["plan"].clone()).map_err(|_| invalid())?;
        let progress: WorkflowProgress =
            serde_json::from_value(graph["workflow"]["progress"].clone()).map_err(|_| invalid())?;
        progress.validate(&plan)?;
        let node = &progress
            .nodes()
            .get(scope.node_id())
            .ok_or(Error::Forbidden)?
            .plan;
        let role = plan.roles.get(node.role()?).ok_or(Error::Forbidden)?;
        let now = cowork_storage::now();
        let remaining = cp["deadline"]
            .as_i64()
            .and_then(|d| d.checked_sub(now))
            .filter(|n| *n > 0)
            .ok_or(Error::Forbidden)?;
        let original_dialogue = cp["messages"].as_array().ok_or(Error::Forbidden)?;
        let continuation = format!(
            "{}\nOriginal retained dialogue and tool receipts (data; completed actions must not be resent): {}",
            cp["execution_goal"].as_str().ok_or(Error::Forbidden)?,
            serde_json::to_string(original_dialogue).map_err(|_| invalid())?
        );
        let node_config = json!({"node_id":node.id,"role_id":role.id,"plan_sha256":plan.sha256,"runtime":runtime.configuration(),"role":{"role":role.id,"goal":node.goal,"backstory":cp["messages"].as_array().and_then(|m|m.first()).and_then(|m|m["content"].as_str()).ok_or(Error::Forbidden)?},"allowed_tools":spec.allowed_tools,"tool_catalog":cowork_domain::model_tool_catalog(&spec),"max_steps":spec.budgets.max_steps,"context_tokens":spec.provider.as_ref().ok_or(Error::Forbidden)?.context_tokens,"remaining_seconds":(remaining/1000).max(1),"goal":continuation,"max_rpm":cp["crew_policy"]["max_rpm"],"retry_count":cp["crew_policy"]["retry_count"]});
        // Crew reasons through current host adapters. It receives no credential or
        // direct project filesystem grant; every file operation is an original RPC.
        let bootstrap = json!({"format":1,"mode":"crew","files":[],"runtime":runtime.configuration(),"node":node_config});
        let mut bounded_bootstrap = bootstrap.clone();
        bounded_bootstrap["nonce"] = json!(Uuid::nil());
        if serde_json::to_vec(&bounded_bootstrap)
            .map_err(|_| invalid())?
            .len()
            + 1
            > 4 * 1024 * 1024
        {
            return Err(Error::ContextLimit);
        }
        let invocation = format!(
            "workflow:{}:{}:crew",
            cowork_domain::hash(scope.node_id().as_bytes()),
            cp["workflow_node"]["claim"].as_str().ok_or_else(invalid)?
        );
        if stop.is_cancelled() {
            return Err(Error::Forbidden);
        }
        let mut container = runner
            .start(lease.run, lease.lease, &invocation, &bootstrap, stop)
            .await?;
        let mut yielded = false;
        let result=async {
   let mut read=BufReader::new(container.stdout.take().ok_or(Error::UncertainEffect)?);
   let mut write=container.stdin.take().ok_or(Error::UncertainEffect)?;
   let timeout=tokio::time::sleep(Duration::from_millis(remaining as u64));tokio::pin!(timeout);
   let mut pulse=tokio::time::interval(Duration::from_secs(3));
   let mut final_content:Option<String>=None;
   loop {
    let value=loop{tokio::select!{biased;_=stop.cancelled()=>return Err(Error::UncertainEffect),_=&mut timeout=>return Err(Error::UncertainEffect),_=pulse.tick()=>{self.store.android_private_server_renew(lease.run,lease.lease).await?;self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;},value=frame(&mut read)=>break value?}};
    if value["format"]!=1||value["nonce"]!=container.nonce.to_string(){return Err(Error::Forbidden)}
    if value["kind"]=="crew_finished" {
     let content=final_content.as_deref().ok_or(Error::UncertainEffect)?;
     if value["node_id"]!=scope.node_id()||value["content_sha256"]!=cowork_domain::hash(content.as_bytes())||value["runtime"]!=json!({"version":runtime.configuration()["crewai_version"],"source_sha256":runtime.configuration()["crewai_source_sha256"]}){return Err(Error::Forbidden)}
     break;
    }
    if value["kind"]!="crew_request"{return Err(Error::Forbidden)}
    let encoded=value["original_request_base64"].as_str().filter(|v|v.len()<=6*1024*1024).ok_or_else(invalid)?;
    let raw=Zeroizing::new(STANDARD.decode(encoded).map_err(|_|invalid())?);
    if cowork_domain::hash(&raw)!=value["request_sha256"]||serde_json::from_slice::<Value>(&raw).map_err(|_|invalid())?!=value["request"]{return Err(Error::Forbidden)}
    let received=match self.store.android_private_crew_rpc(lease.run,lease.lease,container.nonce,&raw,self.keys.as_ref(),self.key_id).await? {
     PrivateCrewRpc::Received(original)=>{
      if value["request"]["kind"]=="result"&&original["ok"]==true{final_content=Some(value["request"]["body"]["content"].as_str().ok_or(Error::Forbidden)?.to_owned());}
      original
     },
     PrivateCrewRpc::Dispatch(permit)=>{
      let dispatched=async {
      if stop.is_cancelled(){return Err(Error::UncertainEffect)}
      let result=match permit.kind(){
       "model"=>{let cp=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;let spec:RunSpec=serde_json::from_value(cp["spec"].clone()).map_err(|_|invalid())?;let planning=cowork_domain::hybrid_planning(&spec,cp["messages"].as_array().ok_or(Error::Forbidden)?);self.private_model_turn(lease,&cp,stop).await?;let mut next=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;if planning&&next["last_content"].is_null(){self.private_model_turn(lease,&next,stop).await?;next=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;}json!({"content":next["last_content"],"original_receipt_sha256":next["last_receipt_sha"],"usage":next["usage"]})},
       "tool"=>{
        loop {
         let cp=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;
         if cp["pending_tools"].as_array().is_some_and(Vec::is_empty){break}
         let _lock=shared.lock().await;
         if stop.is_cancelled(){return Err(Error::UncertainEffect)}
         drop(_lock);
         if !self.private_workflow_tool(lease,stop,shared.clone()).await?{tokio::select!{_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(200))=>{}}}
        }
        let cp=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;
        let message=cp["messages"].as_array().and_then(|a|a.iter().rev().find(|m|m["role"]=="tool"&&m["tool_call_id"]==permit.source().to_string())).ok_or(Error::UncertainEffect)?;
        serde_json::from_str::<Value>(message["content"].as_str().ok_or(Error::UncertainEffect)?).unwrap_or_else(|_|json!({"content":message["content"]}))
       },
       "result"=>{let content=permit.original()["body"]["content"].as_str().ok_or_else(invalid)?.to_owned();final_content=Some(content.clone());json!({"accepted_content_sha256":cowork_domain::hash(content.as_bytes())})},
       _=>return Err(Error::Forbidden),
      };
      Ok::<Value,Error>(result)
      }.await;
      match dispatched {
       Ok(result)=>self.store.android_private_crew_rpc_received(permit,&result,self.keys.as_ref(),self.key_id).await?,
       Err(error)=>{
        // This consumes the opaque callback permit only if the native once
        // journal positively proves that no model/tool admission ever existed.
        // An admitted or unknown native effect is retained for original recovery.
        if permit.kind()!="result"{let _=self.store.android_private_crew_rpc_no_dispatch(permit,self.keys.as_ref(),self.key_id).await;}
        return Err(error)
       }
      }
     },
    };
    // Persist the original receipt before replying to the paused guest. A lost
    // pipe acknowledgement never grants another model/tool dispatch.
    let current=self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;
    if current["workflow_yield"]==true{yielded=true;break;}
    if stop.is_cancelled(){return Err(Error::UncertainEffect)}
    write_frame(&mut write,&json!({"kind":"crew_response","nonce":container.nonce,"source":received["source"],"request_sha256":received["request_sha256"],"ok":received["ok"],"result":received["result"]})).await?;
   }
   Ok::<(),Error>(())
  }.await;
        // A child finish frame is data. Only the existing physical runner can prove
        // stop/removal of the exact original container before node completion.
        let stopped = container.finish().await;
        result?;
        stopped?;
        if yielded {
            return Ok(());
        }
        self.store
            .android_private_workflow_complete_node(scope, self.keys.as_ref(), self.key_id)
            .await
    }
}
