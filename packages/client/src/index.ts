import * as validators from "./validators.js";
import {canonicalJsonValue} from "./canonicalJson";
import {ownerManagedResults,receiveOwnerManagedResult,ownerManagedResultManifest,downloadOwnerManagedResultFile} from "./managedResults";
import {reconcileOwnerManagedProcess} from "./managedProcesses";
import {ownerManagedResultDelivery,prepareOwnerManagedResultDelivery,confirmOwnerManagedResultDelivery,refreshOwnerManagedResultRetirement} from "./managedResultDelivery";
export {canonicalJsonValue} from "./canonicalJson";
import {defaultRunBudgets,type PreparedExecutionPlan,type RunRecord} from "./generated";
import type {DocumentActionRequest,SaveDocumentDraft} from "./generated";
import type { ContractTypes, ApiError, RunRequest, SaveWorkflowInputDraft, WorkflowInputDraftSave, PrepareWorkflowInputAnswer, SaveWorkflowDraft, WorkflowDraftSave, PrepareWorkflowOperation, CreateWorkflow, PublishWorkflow, RevokeWorkflow, CreateProject, CreateOrder, SaveDraft, CreateProvider, ResolveApproval, SubmitInput, BranchOrder, ReconcileFile, CreateRestoreRun, DownloadFile, DownloadMetadata,ReserveUpload,FileUpload,CreateImportRun,ConfigureMcpServer } from "./generated";
export * from "./generated";
export function decode<K extends keyof ContractTypes>(name:K,value:unknown):ContractTypes[K] {
  if(!validators[name](value))throw new Error("contract_mismatch:"+name);
  return value as ContractTypes[K];
}
export class RequestError extends Error {
  constructor(readonly details:ApiError, readonly status:number){super(details.message);}
}
export type DownloadedFile = DownloadMetadata & {blob:Blob};
export async function uploadMetadata(blob:Blob,fileName:string):Promise<DownloadMetadata> {
  if(blob.size>67_108_864)throw new Error("upload_too_large");
  const digest=await crypto.subtle.digest("SHA-256",await blob.arrayBuffer());
  return decode("DownloadMetadata",{file_name:fileName,size:blob.size,sha256:Array.from(new Uint8Array(digest)).map(b=>b.toString(16).padStart(2,"0")).join("")});
}
const sameFile=(a:DownloadMetadata,b:DownloadMetadata)=>a.file_name===b.file_name&&a.size===b.size&&a.sha256===b.sha256;

export async function readDownload(response:Response,expected?:string|null):Promise<DownloadedFile> {
  if(!response.ok)throw new RequestError(decode("ApiError",await response.json()),response.status);
  try {
    if(response.headers.get("content-type")!=="application/octet-stream")throw new Error("download_integrity");
    const size=response.headers.get("content-length");
    if(size===null||!/^(0|[1-9][0-9]*)$/.test(size))throw new Error("download_integrity");
    const encodedName=response.headers.get("x-cowork-file-name");
    if(!encodedName)throw new Error("download_integrity");
    const metadata=decode("DownloadMetadata",{
      file_name:decodeURIComponent(encodedName),size:Number(size),sha256:response.headers.get("x-cowork-sha256")
    });
    if(/[\\/\u0000-\u001f\u007f]/.test(metadata.file_name)||metadata.file_name==="."||metadata.file_name===".."||expected&&expected!==metadata.sha256)throw new Error("download_integrity");
    if(!response.body)throw new Error("download_integrity");
    const reader=response.body.getReader();
    const bytes=new Uint8Array(metadata.size);let offset=0;
    try {
      while(true){const next=await reader.read();if(next.done)break;
        if(offset+next.value.byteLength>bytes.byteLength)throw new Error("download_integrity");
        bytes.set(next.value,offset);offset+=next.value.byteLength;
      }
      if(offset!==metadata.size)throw new Error("download_integrity");
    } finally {await reader.cancel().catch(()=>{});reader.releaseLock();}
    const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");
    if(digest!==metadata.sha256)throw new Error("download_integrity");
    return {...metadata,blob:new Blob([bytes],{type:"application/octet-stream"})};
  } catch(error) {
    if(!response.body?.locked)await response.body?.cancel().catch(()=>{});
    throw error;
  }
}
function canonicalDraftData(value:unknown):unknown {
  if(Array.isArray(value))return value.map(canonicalDraftData);
  if(value!==null&&typeof value==="object")return Object.fromEntries(Object.entries(value).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([key,item])=>[key,canonicalDraftData(item)]));
  return value;
}
const sameDraftData=(a:unknown,b:unknown)=>JSON.stringify(canonicalDraftData(a))===JSON.stringify(canonicalDraftData(b));
async function dataSha256(value:unknown):Promise<string>{
  const bytes=new TextEncoder().encode(JSON.stringify(canonicalDraftData(value)));
  return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(value=>value.toString(16).padStart(2,"0")).join("");
}
async function verifyExecutionInput(value:unknown,order:string):Promise<ContractTypes["PreparedExecutionInput"]>{
  const prepared=decode("PreparedExecutionInput",value), snapshot=prepared.snapshot;
  await verifyWorkspaceTransferManifest(snapshot.manifest);
  if(snapshot.format_version!==1||snapshot.order_id!==order||snapshot.order_revision<=0||snapshot.created_at<=0
    ||snapshot.sources.length>32||snapshot.sources.some((source,index)=>source.alias!==`input-${String(index+1).padStart(4,"0")}`)
    ||prepared.sha256!==await dataSha256(snapshot))throw new Error("execution_input_integrity");
  return prepared;
}
export async function verifyWorkspaceTransferManifest(value:unknown):Promise<ContractTypes["WorkspaceTransferManifest"]>{
  const manifest=decode("WorkspaceTransferManifest",value);
  const {sha256,...payload}=manifest;
  if(manifest.format_version!==1||manifest.total_bytes>512*1024*1024||manifest.files.length+manifest.directories.length>10_000||!/^[a-f0-9]{64}$/.test(sha256))throw new Error("workspace_transfer_manifest_invalid");
  const bytes=new TextEncoder().encode(JSON.stringify(canonicalDraftData(payload)));
  if(bytes.length>2*1024*1024)throw new Error("workspace_transfer_manifest_invalid");
  const actual=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(value=>value.toString(16).padStart(2,"0")).join("");
  if(actual!==sha256)throw new Error("workspace_transfer_manifest_integrity");
  return manifest;
}
const snapshotBrokerTools=new Set(["create_artifact","list_artifacts","read_artifact","request_input","list_files","read_text_file","stat_file","write_text_file","create_directory","rename_file","remove_file","restore_file","delegate_agents"]);
function reviewedBudgetsMatch(request:RunRequest,actual:ContractTypes["Budgets"]):boolean {
  if(!("kind" in request))return sameDraftData({...defaultRunBudgets,...actual},{...defaultRunBudgets,...request.budgets});
  // A published workflow can lower the owner's requested upper limits.
  return (Object.keys(defaultRunBudgets) as (keyof ContractTypes["Budgets"])[]).every(key=>{
    const value=actual[key]??defaultRunBudgets[key],ceiling=request.budgets?.[key]??defaultRunBudgets[key];
    return Number.isSafeInteger(value)&&value>0&&value<=ceiling;
  });
}
async function verifySnapshotPlan(value:unknown):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
  const prepared=decode("PreparedExecutionPlanV2",value),p=prepared.plan;
  const bytes=new TextEncoder().encode(prepared.canonical_json);
  if(bytes.length>8*1024*1024)throw new Error("execution_plan_v2_integrity");
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(v=>v.toString(16).padStart(2,"0")).join("");
  if(bytes.length>8*1024*1024||digest!==prepared.sha256||!sameDraftData(decode("ExecutionPlanV2",JSON.parse(prepared.canonical_json)),p)
    ||p.format_version!==2||p.actor.kind!=="installation_owner"
    ||!Number.isSafeInteger(p.target.controller.generation)||p.target.controller.generation<=0
    ||!Number.isSafeInteger(p.created_at)||!Number.isSafeInteger(p.expires_at)||p.created_at<=0||p.expires_at-p.created_at!==300_000
    ||(p.preflight.kind!=="owner"&&p.preflight.kind!=="restore"&&p.preflight.kind!=="import")||p.boundary.connector_aliases.length!==0
    ||!p.boundary.input_snapshot_bound||p.input.id!==p.workspace.input.snapshot_id||p.input.project_id!==p.project_id
    ||p.input.manifest.sha256!==p.workspace.input.manifest_sha256)throw new Error("execution_plan_v2_integrity");
  await verifyExecutionInput({snapshot:p.input,sha256:p.workspace.input.sha256,retired:false},p.order_id);
  const managed=p.target.kind==="managed_runner",documents=p.target.kind==="document_engine";
  if(p.target.kind==="broker_controller"){
    if(p.boundary.executor!=="local-v2"||p.boundary.process_access!=="broker_only"||p.boundary.process_network.kind!=="no_process"||p.boundary.environment!==null)throw new Error("execution_plan_v2_integrity");
  }else if(p.target.kind==="document_engine"){
    if(p.preflight.kind!=="owner"||p.boundary.executor!=="local-v2"||p.boundary.process_access!=="user_rights"
      ||p.boundary.process_network.kind!=="user_rights"||p.boundary.environment!==null
      ||!/^[a-f0-9]{64}$/.test(p.target.engine_revision))throw new Error("execution_plan_v2_integrity");
  }else{
    const {runner,target,programs}=p.target;
    const key=(bytes:number[])=>bytes.length===32&&bytes.every(b=>Number.isInteger(b)&&b>=0&&b<=255)&&bytes.some(b=>b!==0);
    const relative=(path:string)=>path.length<=512&&path.split("/").every(part=>!!part&&part!=="."&&part!==".."&&!part.endsWith(".")&&/^[A-Za-z0-9._-]+$/.test(part));
    if(p.preflight.kind!=="owner"||!("provider" in p.preflight.review)||p.boundary.process_access!=="qualified_environment"
      ||p.boundary.process_network.kind!=="denied"||!p.boundary.environment||!sameDraftData(p.preflight.review.environment,p.boundary.environment)
      ||p.boundary.executor!==runner.runner_id||!Number.isSafeInteger(runner.generation)||runner.generation<=0
      ||!Number.isSafeInteger(target.revision)||target.revision<=0||!key(runner.control_public_key)||!key(runner.runner_public_key)
      ||sameDraftData(runner.control_public_key,runner.runner_public_key)||!/^[a-f0-9]{64}$/.test(runner.adapter_build_sha256)
      ||!runner.runner_id||runner.runner_id.length>128||!/^[\x21-\x7e]+$/.test(runner.runner_id)
      ||Object.keys(programs).length===0||Object.keys(programs).length>128
      ||Object.entries(programs).some(([name,path])=>!name||name.length>128||!/^[A-Za-z0-9._:-]+$/.test(name)||!relative(path))
      ||p.boundary.folders.some(folder=>!folder.writable)
      ||!p.preflight.review.allowed_tools.includes("execute_program")
      ||target.target_id!==await dataSha256({domain:"execution-target-id/1",control_public_key:runner.control_public_key,runner_id:runner.runner_id})
      ||target.identity_sha256!==await dataSha256({domain:"execution-runner-identity/1",identity:runner}))throw new Error("execution_plan_v2_integrity");
  }
  const flight=p.preflight.review;
  if(!flight.ready||flight.executor!==p.boundary.executor||flight.allowed_tools.some(t=>!snapshotBrokerTools.has(t)&&!(p.preflight.kind==="import"&&t==="import_file")&&!(managed&&t==="execute_program")&&!(documents&&["document_read","document_write","image_edit"].includes(t)))
    ||("folders" in flight&&!sameDraftData(flight.folders??[],p.boundary.folders)))throw new Error("execution_plan_v2_integrity");
  if(p.preflight.kind==="import"){
    const review=p.preflight.review,action=review.import,upload=review.upload;
    if(review.allowed_tools.length!==1||review.allowed_tools[0]!=="import_file"||review.access_mode==="read_only"
      ||upload.state!=="ready"||upload.id!==action.upload_id||upload.order_id!==p.order_id||upload.project_id!==p.project_id
      ||!sameDraftData(upload.file,action.file)||!/^[a-f0-9]{64}$/.test(action.file.sha256)
      ||action.expected_sha256!=null&&!/^[a-f0-9]{64}$/.test(action.expected_sha256)
      ||!review.folders.some(f=>f.id===action.folder_id&&f.writable&&f.order_id===p.order_id))throw new Error("execution_plan_v2_integrity");
  }
  if(p.preflight.kind==="restore"){
    const restore=p.preflight.review;
    if(restore.allowed_tools.length!==1||restore.allowed_tools[0]!=="restore_file"||restore.access_mode==="read_only"
      ||restore.restore.expected_source_sha256!==restore.content.sha256||!/^[a-f0-9]{64}$/.test(restore.content.sha256)
      ||!restore.folders.some(f=>f.id===restore.restore.folder_id&&f.writable))throw new Error("execution_plan_v2_integrity");
  }
  return prepared;
}
export class Client {
  constructor(readonly token:string, readonly base=""){}
  private async request<K extends keyof ContractTypes>(path:string,name:K,method="GET",body?:unknown,array=false):Promise<ContractTypes[K] | ContractTypes[K][]> {
    const response=await fetch(this.base+"/api/v2"+path,{method,headers:{Authorization:"Bearer "+this.token,...(body!==undefined?{"Content-Type":"application/json"}:{})},body:body===undefined?undefined:JSON.stringify(body)});
    const value:unknown=await response.json();
    if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
    if(array){if(!Array.isArray(value))throw new Error("contract_mismatch:array");return value.map(v=>decode(name,v));}
    return decode(name,value);
  }
  one<K extends keyof ContractTypes>(path:string,name:K,method="GET",body?:unknown){return this.request(path,name,method,body) as Promise<ContractTypes[K]>;}
  many<K extends keyof ContractTypes>(path:string,name:K){return this.request(path,name,"GET",undefined,true) as Promise<ContractTypes[K][]>;}
  health=()=>this.one("/health","Health");
  personalMoney=async()=> {
    const status=await this.one("/personal-money","PersonalMoneyStatus");
    if(status.origin!=="not_configured") {
      const pool=status.installation;
      const charged=BigInt(pool.charged_units), limit=BigInt(pool.limit_units);
      const remaining=limit>charged?limit-charged:0n;
      if(charged!==BigInt(pool.confirmed_units)+BigInt(pool.pending_units)+BigInt(pool.unknown_units)
        ||BigInt(pool.remaining_units)!==remaining||pool.limit_exceeded!==(charged>limit)||pool.no_headroom!==(remaining===0n)
        ||(["confirmed","pending","unknown"] as const).some(kind=>pool[`${kind}_attempts`]==="0"&&pool[`${kind}_units`]!=="0"))throw new Error("personal_money_integrity");
    }
    return status;
  };
  providers=()=>this.many("/providers","ProviderProfile");
  folders=(project:string)=>this.many("/folders?project_id="+encodeURIComponent(project),"FolderGrant");
  reconcileFile=(run:string,input:ReconcileFile)=>this.one(`/runs/${encodeURIComponent(run)}/files/reconcile`,"RunRecord","POST",input);
  fileVersion=(run:string,call:string,previous:boolean)=>this.one(`/runs/${encodeURIComponent(run)}/file-changes/${encodeURIComponent(call)}/${previous?"previous":"written"}`,"TextFile");
  createProvider=(input:CreateProvider)=>this.one("/providers","ProviderProfile","POST",input);
  projects=()=>this.many("/projects","Project");
  mcpServers=(project:string)=>this.many(`/projects/${encodeURIComponent(project)}/mcp-servers`,"McpServer");
  configureMcpServer=(project:string,input:ConfigureMcpServer)=>this.one(`/projects/${encodeURIComponent(project)}/mcp-servers`,"McpServer","POST",input);
  mcpAttempts=(run:string)=>this.many(`/runs/${encodeURIComponent(run)}/mcp-attempts`,"McpAttempt");
  createProject=(input:CreateProject)=>this.one("/projects","Project","POST",input);
  workflows=(project:string,offset=0,limit=30)=>this.one(`/projects/${encodeURIComponent(project)}/workflows?`+new URLSearchParams({offset:String(offset),limit:String(limit)}),"WorkflowPage");
  workflow=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflows/${encodeURIComponent(id)}`,"WorkflowLibraryEntry");
  workflowVersion=(project:string,id:string,version:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflows/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}`,"WorkflowVersion");
  createWorkflow=(project:string,input:CreateWorkflow)=>this.one(`/projects/${encodeURIComponent(project)}/workflows`,"WorkflowLibraryEntry","POST",input);
  publishWorkflow=(project:string,id:string,input:PublishWorkflow)=>this.one(`/projects/${encodeURIComponent(project)}/workflows/${encodeURIComponent(id)}/versions`,"WorkflowPublication","POST",input);
  revokeWorkflow=(project:string,id:string,input:RevokeWorkflow)=>this.one(`/projects/${encodeURIComponent(project)}/workflows/${encodeURIComponent(id)}/revoke`,"WorkflowPublication","POST",input);
  workflowDrafts=(project:string,offset=0,limit=20)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-drafts?`+new URLSearchParams({offset:String(offset),limit:String(limit)}),"WorkflowDraftPage");
  workflowDraft=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-drafts/${encodeURIComponent(id)}`,"WorkflowDraft");
  workflowDraftSnapshots=(project:string,id:string,offset=0,limit=20)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-drafts/${encodeURIComponent(id)}/snapshots?`+new URLSearchParams({offset:String(offset),limit:String(limit)}),"WorkflowDraftSnapshotPage");
  workflowDraftSnapshot=(project:string,id:string,snapshot:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-drafts/${encodeURIComponent(id)}/snapshots/${encodeURIComponent(snapshot)}`,"WorkflowDraftSnapshot");
  async saveWorkflowDraft(project:string,input:SaveWorkflowDraft):Promise<WorkflowDraftSave> {
    const body=JSON.stringify(input);
    const frozen=decode("SaveWorkflowDraft",JSON.parse(body));
    const expectedContent={...frozen.content,origin:frozen.content.origin??null};
    const response=await fetch(this.base+`/api/v2/projects/${encodeURIComponent(project)}/workflow-drafts`,{method:"PUT",headers:{Authorization:"Bearer "+this.token,"Content-Type":"application/json"},body});
    const value:unknown=await response.json();
    const isSavedConflict=response.status===409&&typeof value==="object"&&value!==null&&"outcome" in value&&value.outcome==="conflict";
    if(!response.ok&&!isSavedConflict)throw new RequestError(decode("ApiError",value),response.status);
    const result=decode("WorkflowDraftSave",value);
    if(result.draft.project_id!==project||result.draft.id!==frozen.id||result.snapshot.project_id!==project||result.snapshot.draft_id!==frozen.id||result.snapshot.base_revision!==frozen.base_revision
      ||!sameDraftData({...result.snapshot.content,origin:result.snapshot.content.origin??null},expectedContent)
      ||(response.ok&&result.outcome!=="saved")||(isSavedConflict&&result.outcome!=="conflict")||result.snapshot.conflict!==(result.outcome==="conflict")
      ||(result.outcome==="saved"&&(result.draft.snapshot_id!==result.snapshot.id||result.draft.sha256!==result.snapshot.sha256||result.draft.revision!==frozen.base_revision+1||!sameDraftData({...result.draft.content,origin:result.draft.content.origin??null},expectedContent))))throw new Error("workflow_draft_integrity");
    return result;
  }
  workflowOperations=(project:string,offset=0,limit=20,includeAcknowledged=false)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations?`+new URLSearchParams({offset:String(offset),limit:String(limit),include_acknowledged:String(includeAcknowledged)}),"WorkflowOperationPage");
  workflowOperation=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations/${encodeURIComponent(id)}`,"WorkflowOperation");
  prepareWorkflowOperation=(project:string,input:PrepareWorkflowOperation)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations`,"WorkflowOperation","POST",input);
  commitWorkflowOperation=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations/${encodeURIComponent(id)}/commit`,"WorkflowOperation","POST");
  discardWorkflowOperation=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations/${encodeURIComponent(id)}/discard`,"WorkflowOperation","POST");
  acknowledgeWorkflowOperation=(project:string,id:string)=>this.one(`/projects/${encodeURIComponent(project)}/workflow-operations/${encodeURIComponent(id)}/acknowledge`,"WorkflowOperation","POST");
  workflowInputDraft=(run:string,request:string)=>this.one(`/runs/${encodeURIComponent(run)}/workflow/inputs/${encodeURIComponent(request)}/draft`,"WorkflowInputDraftLookup");
  workflowInputDraftSnapshot=(run:string,request:string,snapshot:string)=>this.one(`/runs/${encodeURIComponent(run)}/workflow/inputs/${encodeURIComponent(request)}/draft/snapshots/${encodeURIComponent(snapshot)}`,"WorkflowInputDraftSnapshot");
  prepareWorkflowInputAnswer=(run:string,request:string,input:PrepareWorkflowInputAnswer)=>this.one(`/runs/${encodeURIComponent(run)}/workflow/inputs/${encodeURIComponent(request)}/answer-intents`,"WorkflowInputAnswerIntent","POST",input);
  workflowInputAnswerIntent=(run:string,intent:string)=>this.one(`/runs/${encodeURIComponent(run)}/workflow/input-answer-intents/${encodeURIComponent(intent)}`,"WorkflowInputAnswerIntent");
  commitWorkflowInputAnswerIntent=(run:string,intent:string)=>this.one(`/runs/${encodeURIComponent(run)}/workflow/input-answer-intents/${encodeURIComponent(intent)}/commit`,"WorkflowInputAnswerIntent","POST");
  async saveWorkflowInputDraft(run:string,request:string,input:SaveWorkflowInputDraft):Promise<WorkflowInputDraftSave> {
    const body=JSON.stringify(input), frozen=decode("SaveWorkflowInputDraft",JSON.parse(body));
    const response=await fetch(this.base+`/api/v2/runs/${encodeURIComponent(run)}/workflow/inputs/${encodeURIComponent(request)}/draft`,{method:"PUT",headers:{Authorization:"Bearer "+this.token,"Content-Type":"application/json"},body});
    const value:unknown=await response.json();
    const conflict=response.status===409&&typeof value==="object"&&value!==null&&"outcome" in value&&value.outcome==="conflict";
    if(!response.ok&&!conflict)throw new RequestError(decode("ApiError",value),response.status);
    const result=decode("WorkflowInputDraftSave",value), {draft,snapshot}=result;
    const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(frozen.answer)))).map(b=>b.toString(16).padStart(2,"0")).join("");
    if(draft.run_id!==run||snapshot.run_id!==run||draft.request_id!==request||snapshot.request_id!==request||draft.fingerprint!==frozen.fingerprint||snapshot.fingerprint!==frozen.fingerprint
      ||draft.owner!=="single_owner_preview"||snapshot.owner!==draft.owner||snapshot.project_id!==draft.project_id||snapshot.base_revision!==frozen.base_revision||snapshot.answer!==frozen.answer||snapshot.answer_sha256!==digest
      ||(response.ok&&result.outcome!=="saved")||(conflict&&result.outcome!=="conflict")||snapshot.conflict!==(result.outcome==="conflict")
      ||(result.outcome==="saved"&&(draft.snapshot_id!==snapshot.id||draft.answer!==frozen.answer||draft.answer_sha256!==digest||draft.revision!==frozen.base_revision+1||snapshot.revision!==draft.revision))
      ||(result.outcome==="conflict"&&(snapshot.revision!==draft.revision||draft.revision===frozen.base_revision)))throw new Error("workflow_input_draft_integrity");
    return result;
  }
  workflowRun=(run:string)=>this.one(`/runs/${encodeURIComponent(run)}/workflow`,"WorkflowRunView");
  orders=(project:string)=>this.many("/orders?project_id="+encodeURIComponent(project),"Order");
  createOrder=(input:CreateOrder)=>this.one("/orders","Order","POST",input);
  order=(id:string)=>this.one("/orders/"+encodeURIComponent(id),"Order");
  saveDraft=(id:string,input:SaveDraft)=>this.one("/orders/"+encodeURIComponent(id)+"/draft","Order","PUT",input);
  messages=(id:string)=>this.many("/orders/"+encodeURIComponent(id)+"/messages","Message");
  workspace=(id:string)=>this.one(`/orders/${encodeURIComponent(id)}/workspace`,"Workspace");
  async prepareDocumentAction(order:string,input:DocumentActionRequest){
    const result=await this.one(`/orders/${encodeURIComponent(order)}/document-actions/prepare`,"DocumentPreparation","POST",input);
    if(result.order_id!==order||result.idempotency_key!==input.idempotency_key||result.access_mode!==input.access_mode||result.tool.name!==input.tool.name)throw new Error("document_preparation_integrity");
    return result;
  }
  async documentPreparationByKey(order:string,key:string){
    const result=await this.one(`/orders/${encodeURIComponent(order)}/document-actions/by-key/${encodeURIComponent(key)}`,"DocumentPreparation");
    if(result.order_id!==order||result.idempotency_key!==key)throw new Error("document_preparation_integrity");return result;
  }
  async commitDocumentAction(order:string,preparation:string){
    const result=await this.one(`/orders/${encodeURIComponent(order)}/document-actions/${encodeURIComponent(preparation)}/commit`,"RunRecord","POST");
    if(result.spec.order_id!==order||result.spec.provider!=null||!result.spec.file_action)throw new Error("document_run_integrity");return result;
  }
  documentDraft=(order:string,id:string)=>this.one(`/orders/${encodeURIComponent(order)}/document-drafts/${encodeURIComponent(id)}`,"DocumentDraft");
  documentDraftSnapshots=(order:string,id:string)=>this.many(`/orders/${encodeURIComponent(order)}/document-drafts/${encodeURIComponent(id)}/snapshots`,"DocumentDraftSnapshot");
  async saveDocumentDraft(order:string,input:SaveDocumentDraft){
    const result=await this.one(`/orders/${encodeURIComponent(order)}/document-drafts`,"DocumentDraftSave","PUT",input);
    if(result.draft.order_id!==order||result.draft.id!==input.id||result.snapshot.draft_id!==input.id||result.snapshot.order_id!==order
      ||result.snapshot.base_revision!==input.base_revision||!sameDraftData(result.snapshot.content,input.content)
      ||result.snapshot.conflict===result.saved||(result.saved&&(result.draft.revision!==input.base_revision+1||!sameDraftData(result.draft.content,input.content))))throw new Error("document_draft_integrity");
    return result;
  }
  workspaceFiles=(id:string,path=".")=>this.one(`/orders/${encodeURIComponent(id)}/workspace/files?`+new URLSearchParams({path}),"DirectoryListing");
  runCopyFiles=(order:string,run:string,location:ContractTypes["FileLocation"])=>this.one(`/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/working-copy/files?`+new URLSearchParams(location),"DirectoryListing");
  runCopyText=(order:string,run:string,location:ContractTypes["FileLocation"])=>this.one(`/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/working-copy/text?`+new URLSearchParams(location),"TextFile");
  workspaceText=(id:string,path:string)=>this.one(`/orders/${encodeURIComponent(id)}/workspace/text?`+new URLSearchParams({path}),"TextFile");
  workspaceStat=(id:string,path:string)=>this.one(`/orders/${encodeURIComponent(id)}/workspace/stat?`+new URLSearchParams({path}),"FileStat");
  uploads=(id:string,offset=0)=>this.one(`/orders/${encodeURIComponent(id)}/uploads?offset=${offset}`,"UploadPage");
  async reserveUpload(order:string,input:ReserveUpload):Promise<FileUpload> {
    const result=await this.one(`/orders/${encodeURIComponent(order)}/uploads`,"FileUpload","POST",input);
    if(result.order_id!==order||!sameFile(result.file,input.file))throw new Error("upload_integrity");
    return result;
  }
  async uploadContent(order:string,upload:FileUpload,blob:Blob):Promise<FileUpload> {
    if(upload.order_id!==order||!sameFile(await uploadMetadata(blob,upload.file.file_name),upload.file))throw new Error("upload_integrity");
    const response=await fetch(this.base+`/api/v2/orders/${encodeURIComponent(order)}/uploads/${encodeURIComponent(upload.id)}/content`,{method:"PUT",headers:{Authorization:"Bearer "+this.token,"Content-Type":"application/octet-stream"},body:blob});
    const value:unknown=await response.json();
    if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
    const result=decode("FileUpload",value);
    if(result.id!==upload.id||result.order_id!==order||result.state!=="ready"||!sameFile(result.file,upload.file))throw new Error("upload_integrity");
    return result;
  }
  importFiles=(input:CreateImportRun)=>this.one("/file-imports","RunRecord","POST",input);
  async downloadWorkspace(order:string,input:DownloadFile,run?:string):Promise<DownloadedFile> {
    const query=new URLSearchParams({path:input.path,...(input.expected_sha256?{expected_sha256:input.expected_sha256}:{})});
    const path=`/orders/${encodeURIComponent(order)}${run?`/runs/${encodeURIComponent(run)}`:""}/workspace/download?${query}`;
    return readDownload(await fetch(this.base+"/api/v2"+path,{headers:{Authorization:"Bearer "+this.token}}),input.expected_sha256);
  }
  async workspaceTransferManifest(order:string,run:string):Promise<ContractTypes["WorkspaceTransferManifest"]> {
    return verifyWorkspaceTransferManifest(await this.one(`/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/workspace/transfer`,"WorkspaceTransferManifest"));
  }
  async downloadWorkspaceTransferChunk(order:string,run:string,manifest:ContractTypes["WorkspaceTransferManifest"],path:string,offset:number):Promise<DownloadedFile> {
    const frozen=await verifyWorkspaceTransferManifest(structuredClone(manifest));
    const chunk=frozen.files.find(file=>file.path===path)?.chunks.find(chunk=>chunk.offset===offset);
    if(!chunk||!Number.isSafeInteger(offset)||chunk.size<=0||chunk.size>4*1024*1024)throw new Error("workspace_transfer_chunk_invalid");
    const query=new URLSearchParams({path,offset:String(offset),expected_sha256:chunk.sha256});
    const response=await fetch(this.base+`/api/v2/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/workspace/transfer/chunk?${query}`,{headers:{Authorization:"Bearer "+this.token}});
    if(response.ok&&response.headers.get("content-length")!==String(chunk.size)){await response.body?.cancel();throw new Error("download_integrity");}
    return readDownload(response,chunk.sha256);
  }
  ownerManagedResults=(order:string,run:string)=>ownerManagedResults(this,order,run);
  ownerManagedResultDelivery=(order:string,run:string,attempt:string)=>ownerManagedResultDelivery(this,order,run,attempt);
  prepareOwnerManagedResultDelivery=(order:string,run:string,attempt:string)=>prepareOwnerManagedResultDelivery(this,order,run,attempt);
  confirmOwnerManagedResultDelivery=(order:string,run:string,attempt:string,input:ContractTypes["ConfirmOwnerManagedResultDelivery"])=>confirmOwnerManagedResultDelivery(this,order,run,attempt,input);
  refreshOwnerManagedResultRetirement=(order:string,run:string,attempt:string)=>refreshOwnerManagedResultRetirement(this,order,run,attempt);
  reconcileOwnerManagedProcess=(order:string,run:string,attempt:string,input:ContractTypes["ReconcileOwnerManagedProcess"])=>reconcileOwnerManagedProcess(this,order,run,attempt,input);
  receiveOwnerManagedResult=(order:string,run:string,attempt:string)=>receiveOwnerManagedResult(this,order,run,attempt);
  ownerManagedResultManifest=(order:string,run:string,version:string,signal?:AbortSignal)=>ownerManagedResultManifest(this,order,run,version,signal);
  downloadOwnerManagedResultFile=(order:string,run:string,result:ContractTypes["OwnerManagedResultManifest"],path:string,signal?:AbortSignal)=>downloadOwnerManagedResultFile(this,order,run,result,path,signal);
  history=(id:string)=>this.one("/orders/"+encodeURIComponent(id)+"/history","OrderHistory");
  exportOrder=(id:string)=>this.one("/orders/"+encodeURIComponent(id)+"/export","OrderExport");
  branchOrder=(id:string,input:BranchOrder)=>this.one("/orders/"+encodeURIComponent(id)+"/branches","Order","POST",input);
  activity=(attention=false,offset=0)=>this.one(`/activity?attention_only=${attention}&offset=${offset}`,"ActivityPage");
  search=(query:string,project="",offset=0)=>this.one("/search?"+new URLSearchParams({q:query,...(project?{project_id:project}:{}),offset:String(offset)}),"SearchPage");
  runs=(id:string)=>this.many("/orders/"+encodeURIComponent(id)+"/runs","RunRecord");
  async workflowInputSnapshot(run:string):Promise<ContractTypes["WorkflowInputSnapshot"]>{
    const snapshot=await this.one(`/runs/${encodeURIComponent(run)}/workflow/input-snapshot`,"WorkflowInputSnapshot");
    if(snapshot.source.run_id!==run||!/^[a-f0-9]{64}$/.test(snapshot.source.sha256)
      ||new TextEncoder().encode(snapshot.canonical_json).length>2*1024*1024)throw new Error("workflow_input_source_mismatch");
    JSON.parse(snapshot.canonical_json);
    const bytes=new TextEncoder().encode(snapshot.canonical_json);
    const sha=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(v=>v.toString(16).padStart(2,"0")).join("");
    if(sha!==snapshot.source.sha256)throw new Error("workflow_input_source_mismatch");
    return snapshot;
  }
  artifacts=(id:string)=>this.many("/orders/"+encodeURIComponent(id)+"/artifacts","Artifact");
  preflight=(input:RunRequest)=>this.one("/preflight","RunPreflight","POST",input);
  restoreFiles=(input:CreateRestoreRun)=>this.one("/file-restorations","RunRecord","POST",input);
  async prepareExecutionInput(order:string,input:ContractTypes["PrepareExecutionInput"]):Promise<ContractTypes["PreparedExecutionInput"]>{
    const request=decode("PrepareExecutionInput",structuredClone(input));
    if(!Number.isSafeInteger(request.base_revision)||request.base_revision<=0||request.folder_ids.length>32
      ||request.folder_ids.some((id,index)=>!id||index>0&&id<=request.folder_ids[index-1]))throw new Error("execution_input_selection_invalid");
    const prepared=await verifyExecutionInput(await this.one(`/orders/${encodeURIComponent(order)}/execution-inputs`,"PreparedExecutionInput","POST",request),order);
    const expected=await dataSha256({domain:"execution-input-request/1",order_id:order,request});
    if(prepared.snapshot.request_sha256!==expected||prepared.snapshot.order_revision!==request.base_revision||prepared.snapshot.sources.length!==request.folder_ids.length)throw new Error("execution_input_response_mismatch");
    return prepared;
  }
  async inspectExecutionInput(input:ContractTypes["PreparedExecutionInput"]):Promise<ContractTypes["PreparedExecutionInput"]>{
    const original=await verifyExecutionInput(structuredClone(input),input.snapshot.order_id);
    const current=await verifyExecutionInput(await this.one(`/orders/${encodeURIComponent(original.snapshot.order_id)}/execution-inputs/${encodeURIComponent(original.snapshot.id)}`,"PreparedExecutionInput"),original.snapshot.order_id);
    if(!sameDraftData(current.snapshot,original.snapshot)||current.sha256!==original.sha256||original.retired&&!current.retired)throw new Error("execution_input_receipt_mismatch");
    return current;
  }
  async retireExecutionInput(input:ContractTypes["PreparedExecutionInput"]):Promise<ContractTypes["PreparedExecutionInput"]>{
    const original=await verifyExecutionInput(structuredClone(input),input.snapshot.order_id);
    const retired=await verifyExecutionInput(await this.one(`/orders/${encodeURIComponent(original.snapshot.order_id)}/execution-inputs/${encodeURIComponent(original.snapshot.id)}/retire`,"PreparedExecutionInput","POST",{sha256:original.sha256}),original.snapshot.order_id);
    if(!retired.retired||retired.sha256!==original.sha256||!sameDraftData(retired.snapshot,original.snapshot))throw new Error("execution_input_receipt_mismatch");
    return retired;
  }
  async prepareExecutionPlan(input:RunRequest):Promise<PreparedExecutionPlan> {
    const request=decode("RunRequest",structuredClone(input));
    const prepared=await this.one("/execution-plans","PreparedExecutionPlan","POST",request);
    if(prepared.plan.format_version!==1||prepared.plan.actor!=="installation-owner"
      ||prepared.plan.order_id!==request.order_id||prepared.plan.idempotency_key!==request.idempotency_key
      ||!/^[a-f0-9]{64}$/.test(prepared.sha256)||prepared.plan.expires_at-prepared.plan.created_at!==300_000)
      throw new Error("execution_plan_response_mismatch");
    const flight=prepared.plan.preflight;
    if("kind" in request){
      if(!("workflow" in flight)||flight.workflow.workflow_id!==request.workflow_id||flight.workflow.version_id!==request.version_id
        ||request.expected_binding&&!sameDraftData(request.expected_binding,flight.workflow))throw new Error("execution_plan_response_mismatch");
    }else if(!("provider" in flight)||flight.provider.id!==request.provider_id||flight.provider.revision!==request.provider_revision){
      throw new Error("execution_plan_response_mismatch");
    }
    if(!sameDraftData(prepared.plan.boundary.environment,request.environment??null)
      ||prepared.plan.boundary.process_access==="user_rights"&&!request.native_tools?.length
      ||prepared.plan.boundary.folders.some(folder=>{
        const selected=request.folders?.find(item=>item.id===folder.id);
        return selected?!sameDraftData({...folder,writable:selected.writable},selected)||folder.writable&&!selected.writable:folder.order_id!==request.order_id;
      }))throw new Error("execution_plan_response_mismatch");
    return prepared;
  }
  async prepareExecutionPlanV2(input:ContractTypes["PrepareOwnerExecutionPlanV2"]):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
    const request=decode("PrepareOwnerExecutionPlanV2",structuredClone(input));
    const prepared=await verifySnapshotPlan(await this.one("/execution-plans/v2","PreparedExecutionPlanV2","POST",request));
    const p=prepared.plan,run=request.request;
    if(p.preflight.kind!=="owner"||p.order_id!==run.order_id||p.idempotency_key!==run.idempotency_key
      ||!sameDraftData(p.workspace.input,request.input))throw new Error("execution_plan_v2_response_mismatch");
    const flight=p.preflight.review;
    if((run.native_tools?.length??0)>0||(run.mcp_tools?.length??0)>0
      ||(request.target.kind==="registered_runner")!==(p.target.kind==="managed_runner")
      ||(request.target.kind==="document_engine")!==(p.target.kind==="document_engine")
      ||!sameDraftData(run.environment??null,p.boundary.environment)
      ||!({read_only:["read_only"],confirm_all:["read_only","confirm_all"],confirm_mutations:["read_only","confirm_all","confirm_mutations"],autonomous:["read_only","confirm_all","confirm_mutations","autonomous"]}[run.access_mode??"confirm_mutations"]).includes(flight.access_mode)
      ||!reviewedBudgetsMatch(run,flight.budgets)
      ||p.boundary.folders.some(folder=>{
        const selected=run.folders?.find(f=>f.id===folder.id);
        return selected?folder.name!==selected.name||folder.project_id!==selected.project_id||(folder.order_id??null)!==(selected.order_id??null)||folder.writable&&!selected.writable:folder.order_id!==run.order_id;
      }))throw new Error("execution_plan_v2_response_mismatch");
    if("kind" in run){
      if(!("workflow" in flight)||flight.workflow.workflow_id!==run.workflow_id||flight.workflow.version_id!==run.version_id
        ||run.expected_binding&&!sameDraftData(run.expected_binding,flight.workflow)
        ||!sameDraftData(flight.input_source??null,run.input_source??null))throw new Error("execution_plan_v2_response_mismatch");
      if(run.input_source){
        if(run.inputs!=null||run.expected_binding!=null)throw new Error("workflow_input_source_mismatch");
        const raw=canonicalJsonValue(prepared.canonical_json,["preflight","review","workflow","inputs"]);
        const sha=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(raw)))).map(v=>v.toString(16).padStart(2,"0")).join("");
        if(sha!==run.input_source.sha256)throw new Error("workflow_input_source_mismatch");
      }else if(!sameDraftData(flight.workflow.inputs??null,run.inputs??null))throw new Error("execution_plan_v2_response_mismatch");
    }else if(!("provider" in flight)||flight.provider.id!==run.provider_id||flight.provider.revision!==run.provider_revision)throw new Error("execution_plan_v2_response_mismatch");
    return prepared;
  }
  async prepareImportExecutionPlanV2(input:ContractTypes["PrepareImportExecutionPlanV2"]):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
    const request=decode("PrepareImportExecutionPlanV2",structuredClone(input));
    const prepared=await verifySnapshotPlan(await this.one("/execution-plans/v2/file-imports","PreparedExecutionPlanV2","POST",request));
    const p=prepared.plan,r=request.request;
    if(p.preflight.kind!=="import"||p.order_id!==r.order_id||p.idempotency_key!==r.idempotency_key
      ||!sameDraftData(p.workspace.input,request.input))throw new Error("execution_plan_v2_response_mismatch");
    const review=p.preflight.review;
    const normalized={...request,request:{...r,access_mode:r.access_mode??"confirm_mutations",import:{...r.import,expected_sha256:r.import.expected_sha256??null}}};
    if(p.request_sha256!==await dataSha256(normalized)||!sameDraftData(review.import,normalized.request.import)
      ||review.access_mode!==(r.access_mode??"confirm_mutations")
      ||!sameDraftData({...defaultRunBudgets,...review.budgets},defaultRunBudgets))throw new Error("execution_plan_v2_response_mismatch");
    return prepared;
  }
  async retireImportExecutionRequestV2(input:ContractTypes["PrepareImportExecutionPlanV2"]){
    const request=decode("PrepareImportExecutionPlanV2",structuredClone(input));
    const receipt=await this.one("/execution-plans/v2/file-imports/retire-request","RetiredExecutionRequest","POST",request);
    if(receipt.order_id!==request.request.order_id||receipt.idempotency_key!==request.request.idempotency_key||!receipt.retired)throw new Error("execution_plan_v2_receipt_mismatch");
    return receipt;
  }
  async retireImportExecutionIntentV2(input:CreateImportRun){
    const request=decode("CreateImportRun",structuredClone(input));
    const receipt=await this.one("/execution-plans/v2/file-imports/retire-intent","RetiredExecutionRequest","POST",request);
    if(receipt.order_id!==request.order_id||receipt.idempotency_key!==request.idempotency_key||!receipt.retired)throw new Error("execution_plan_v2_receipt_mismatch");
    return receipt;
  }
  async commitImportExecutionPlanV2(prepared:ContractTypes["PreparedExecutionPlanV2"]):Promise<RunRecord>{
    const original=await verifySnapshotPlan(structuredClone(prepared)),p=original.plan;
    if(p.preflight.kind!=="import")throw new Error("execution_plan_v2_receipt_mismatch");
    const review=p.preflight.review;
    const run=await this.one(`/orders/${encodeURIComponent(p.order_id)}/execution-plans/v2/${encodeURIComponent(p.id)}/commit`,"RunRecord","POST",{sha256:original.sha256});
    if(run.spec.order_id!==p.order_id||run.spec.project_id!==p.project_id||run.spec.executor!==p.boundary.executor
      ||!sameDraftData(run.spec.input_workspace,p.workspace)||!sameDraftData(run.spec.folders??[],p.boundary.folders)
      ||run.spec.access_mode!==review.access_mode||!sameDraftData(run.spec.allowed_tools,review.allowed_tools)
      ||!sameDraftData({...defaultRunBudgets,...run.spec.budgets},{...defaultRunBudgets,...review.budgets})
      ||run.spec.provider!=null||run.spec.workflow!=null||run.spec.model_binding!=null||run.spec.agent_config!=null
      ||run.spec.output!=null||run.spec.environment!=null||(run.spec.mcp_tools?.length??0)!==0
      ||run.spec.file_action?.id!=="import"||run.spec.file_action.name!=="import_file"
      ||!sameDraftData(run.spec.file_action.arguments,review.import)||await dataSha256(run.spec)!==p.spec_sha256
      ||original.committed_run_id!==null&&run.id!==original.committed_run_id)throw new Error("execution_plan_v2_receipt_mismatch");
    return run;
  }
  async prepareRestoreExecutionPlanV2(input:ContractTypes["PrepareRestoreExecutionPlanV2"]):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
    const request=decode("PrepareRestoreExecutionPlanV2",structuredClone(input));
    const prepared=await verifySnapshotPlan(await this.one("/execution-plans/v2/file-restorations","PreparedExecutionPlanV2","POST",request));
    const p=prepared.plan,r=request.request;
    if(p.preflight.kind!=="restore"||p.order_id!==r.order_id||p.idempotency_key!==r.idempotency_key
      ||!sameDraftData(p.workspace.input,request.input))throw new Error("execution_plan_v2_response_mismatch");
    const review=p.preflight.review;
    if(r.restore.expected_source_sha256!=null&&r.restore.expected_source_sha256!==review.content.sha256
      ||!sameDraftData(review.restore,{...r.restore,expected_source_sha256:review.content.sha256})
      ||review.access_mode!==(r.access_mode??"confirm_mutations")
      ||!sameDraftData({...defaultRunBudgets,...review.budgets},defaultRunBudgets))throw new Error("execution_plan_v2_response_mismatch");
    return prepared;
  }
  retireRestoreExecutionRequestV2=(request:ContractTypes["PrepareRestoreExecutionPlanV2"])=>this.one("/execution-plans/v2/file-restorations/retire-request","RetiredExecutionRequest","POST",decode("PrepareRestoreExecutionPlanV2",request));
  retireRestoreExecutionIntentV2=(request:CreateRestoreRun)=>this.one("/execution-plans/v2/file-restorations/retire-intent","RetiredExecutionRequest","POST",decode("CreateRestoreRun",request));
  async commitRestoreExecutionPlanV2(prepared:ContractTypes["PreparedExecutionPlanV2"]):Promise<RunRecord>{
    const original=await verifySnapshotPlan(structuredClone(prepared)),p=original.plan;
    if(p.preflight.kind!=="restore")throw new Error("execution_plan_v2_receipt_mismatch");
    const review=p.preflight.review;
    const run=await this.one(`/orders/${encodeURIComponent(p.order_id)}/execution-plans/v2/${encodeURIComponent(p.id)}/commit`,"RunRecord","POST",{sha256:original.sha256});
    if(run.spec.order_id!==p.order_id||run.spec.project_id!==p.project_id||run.spec.executor!==p.boundary.executor
      ||!sameDraftData(run.spec.input_workspace,p.workspace)||!sameDraftData(run.spec.folders??[],p.boundary.folders)
      ||run.spec.access_mode!==review.access_mode||!sameDraftData(run.spec.allowed_tools,review.allowed_tools)
      ||!sameDraftData({...defaultRunBudgets,...run.spec.budgets},{...defaultRunBudgets,...review.budgets})
      ||run.spec.provider!=null||run.spec.workflow!=null||run.spec.model_binding!=null||run.spec.agent_config!=null
      ||run.spec.output!=null||run.spec.environment!=null||(run.spec.mcp_tools?.length??0)!==0
      ||run.spec.file_action?.id!=="restore"||run.spec.file_action.name!=="restore_file"
      ||!sameDraftData(run.spec.file_action.arguments,review.restore)||await dataSha256(run.spec)!==p.spec_sha256
      ||original.committed_run_id!==null&&run.id!==original.committed_run_id)throw new Error("execution_plan_v2_receipt_mismatch");
    return run;
  }
  async inspectExecutionPlanV2(prepared:ContractTypes["PreparedExecutionPlanV2"]):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
    const original=await verifySnapshotPlan(structuredClone(prepared));
    const result=await verifySnapshotPlan(await this.one(`/orders/${encodeURIComponent(original.plan.order_id)}/execution-plans/v2/${encodeURIComponent(original.plan.id)}`,"PreparedExecutionPlanV2"));
    if(result.sha256!==original.sha256||result.canonical_json!==original.canonical_json
      ||original.committed_run_id!==null&&result.committed_run_id!==original.committed_run_id||original.retired&&!result.retired)throw new Error("execution_plan_v2_receipt_mismatch");
    return result;
  }
  async commitExecutionPlanV2(prepared:ContractTypes["PreparedExecutionPlanV2"]):Promise<RunRecord>{
    const original=await verifySnapshotPlan(structuredClone(prepared)),p=original.plan;
    const run=await this.one(`/orders/${encodeURIComponent(p.order_id)}/execution-plans/v2/${encodeURIComponent(p.id)}/commit`,"RunRecord","POST",{sha256:original.sha256});
    if(p.preflight.kind!=="owner")throw new Error("execution_plan_v2_receipt_mismatch");
    const flight=p.preflight.review;
    if(run.spec.order_id!==p.order_id||run.spec.project_id!==p.project_id||run.spec.executor!==p.boundary.executor
      ||!sameDraftData(run.spec.input_workspace,p.workspace)||!sameDraftData(run.spec.folders??[],p.boundary.folders)
      ||run.spec.access_mode!==flight.access_mode||!sameDraftData(run.spec.allowed_tools,flight.allowed_tools)
      ||!sameDraftData(run.spec.environment??null,p.boundary.environment)||(run.spec.mcp_tools?.length??0)!==0
      ||!sameDraftData({...defaultRunBudgets,...run.spec.budgets},{...defaultRunBudgets,...flight.budgets})
      ||("provider" in flight?!sameDraftData(run.spec.provider,flight.provider)||run.spec.workflow!=null:!sameDraftData(run.spec.workflow,flight.workflow))
      ||original.committed_run_id!==null&&run.id!==original.committed_run_id)throw new Error("execution_plan_v2_receipt_mismatch");
    return run;
  }
  async retireExecutionPlanV2(prepared:ContractTypes["PreparedExecutionPlanV2"]):Promise<ContractTypes["PreparedExecutionPlanV2"]>{
    const original=await verifySnapshotPlan(structuredClone(prepared));
    const result=await verifySnapshotPlan(await this.one(`/orders/${encodeURIComponent(original.plan.order_id)}/execution-plans/v2/${encodeURIComponent(original.plan.id)}/retire`,"PreparedExecutionPlanV2","POST",{sha256:original.sha256}));
    if(result.sha256!==original.sha256||result.canonical_json!==original.canonical_json||!result.retired||result.committed_run_id!==null)throw new Error("execution_plan_v2_receipt_mismatch");
    return result;
  }
  async retireExecutionRequestV2(input:ContractTypes["PrepareOwnerExecutionPlanV2"]){
    const request=decode("PrepareOwnerExecutionPlanV2",structuredClone(input));
    const result=await this.one("/execution-plans/v2/retire-request","RetiredExecutionRequest","POST",request);
    if(result.order_id!==request.request.order_id||result.idempotency_key!==request.request.idempotency_key||!result.retired)throw new Error("execution_plan_v2_receipt_mismatch");
    return result;
  }
  inspectExecutionPlan=(prepared:PreparedExecutionPlan)=>this.one(`/orders/${encodeURIComponent(prepared.plan.order_id)}/execution-plans/${encodeURIComponent(prepared.plan.id)}`,"PreparedExecutionPlan");
  async commitExecutionPlan(prepared:PreparedExecutionPlan):Promise<RunRecord> {
    const original=decode("PreparedExecutionPlan",structuredClone(prepared));
    const run=await this.one(`/orders/${encodeURIComponent(original.plan.order_id)}/execution-plans/${encodeURIComponent(original.plan.id)}/commit`,"RunRecord","POST",{sha256:original.sha256});
    if(run.spec.order_id!==original.plan.order_id||run.spec.project_id!==original.plan.project_id
      ||run.spec.executor!==original.plan.boundary.executor
      ||run.spec.access_mode!==original.plan.preflight.access_mode
      ||run.spec.allowed_tools.some(tool=>!original.plan.preflight.allowed_tools.includes(tool))
      ||original.committed_run_id!==null&&run.id!==original.committed_run_id)throw new Error("execution_plan_receipt_mismatch");
    return run;
  }
  async retireExecutionPlan(prepared:PreparedExecutionPlan):Promise<PreparedExecutionPlan> {
    const original=decode("PreparedExecutionPlan",structuredClone(prepared));
    const retired=await this.one(`/orders/${encodeURIComponent(original.plan.order_id)}/execution-plans/${encodeURIComponent(original.plan.id)}/retire`,"PreparedExecutionPlan","POST",{sha256:original.sha256});
    if(retired.sha256!==original.sha256||retired.plan.id!==original.plan.id||!retired.retired||retired.committed_run_id!==null)throw new Error("execution_plan_retirement_mismatch");
    return retired;
  }
  async retireExecutionRequest(input:RunRequest){
    const request=decode("RunRequest",structuredClone(input));
    const receipt=await this.one("/execution-plans/retire-request","RetiredExecutionRequest","POST",request);
    if(receipt.order_id!==request.order_id||receipt.idempotency_key!==request.idempotency_key||!receipt.retired)throw new Error("execution_plan_retirement_mismatch");
    return receipt;
  }
  /** Explicit start callers still use one method; all new dispatches confirm a durable plan. */
  async createRun(input:RunRequest):Promise<RunRecord> {
    return this.commitExecutionPlan(await this.prepareExecutionPlan(input));
  }
  detail=(id:string)=>this.one("/runs/"+encodeURIComponent(id),"RunDetail");
  events=(id:string,after:number)=>this.many("/runs/"+encodeURIComponent(id)+"/events?after="+after,"RunEvent");
  cancel=(id:string)=>this.one("/runs/"+encodeURIComponent(id)+"/cancel","RunRecord","POST",{});
  resume=(id:string)=>this.one("/runs/"+encodeURIComponent(id)+"/resume","RunRecord","POST",{});
  approve=(run:string,id:string,input:ResolveApproval)=>this.one("/runs/"+encodeURIComponent(run)+"/approvals/"+encodeURIComponent(id),"RunRecord","POST",input);
  submitInput=(run:string,input:SubmitInput)=>this.one("/runs/"+encodeURIComponent(run)+"/input","RunRecord","POST",input);
}
