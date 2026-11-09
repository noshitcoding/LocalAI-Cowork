import type {Client} from "@cowork/client";
import {productRequest,type ToolJob,type ToolPreview,type ToolRequest} from "./productApi";
export type ToolboxComponent="browser"|"github"|"office"|"git"|"pull_request";
export type PendingToolboxAction={format:1,endpoint:string,order:string,component:ToolboxComponent,request:ToolRequest,preview:ToolPreview};
export type PendingStorage=Pick<Storage,"getItem"|"setItem"|"removeItem">;
export function toolboxEndpoint(base:string){
  if(!base)return "same-origin";
  if(base.startsWith("/"))return "same-origin"+base.replace(/\/$/,"");
  const url=new URL(base);return url.protocol+"//"+url.host+url.pathname.replace(/\/$/,"");
}
function pendingKey(endpoint:string,order:string,component:ToolboxComponent){return "cowork-v2:toolbox-pending:v1:"+encodeURIComponent(JSON.stringify([endpoint,order,component]));}
export function toolboxPendingKey(base:string,order:string,component:ToolboxComponent){return pendingKey(toolboxEndpoint(base),order,component);}
function validate(value:unknown,base:string,order:string,component:ToolboxComponent):PendingToolboxAction{
  const record=value as PendingToolboxAction;
  if(!record||record.format!==1||record.endpoint!==toolboxEndpoint(base)||record.order!==order||record.component!==component||!record.request||typeof record.request.idempotency_key!=="string"||!record.request.idempotency_key||record.request.action?.kind!==component||!Number.isFinite(record.request.timeout_seconds)||!Number.isFinite(record.request.output_bytes)||!record.preview||!/^[0-9a-f]{64}$/.test(record.preview.request_sha256)||typeof record.preview.cwd!=="string"||!Number.isFinite(record.preview.timeout_seconds))throw new Error("pending_toolbox_journal_invalid");
  return record;
}
export function readPendingToolbox(storage:PendingStorage,base:string,order:string,component:ToolboxComponent){
  const raw=storage.getItem(toolboxPendingKey(base,order,component));if(raw===null)return null;
  if(raw.length>8_388_608)throw new Error("pending_toolbox_journal_invalid");
  try{return validate(JSON.parse(raw),base,order,component);}catch{throw new Error("pending_toolbox_journal_invalid");}
}
export function savePendingToolbox(storage:PendingStorage,base:string,order:string,component:ToolboxComponent,request:ToolRequest,preview:ToolPreview){
  const record:PendingToolboxAction={format:1,endpoint:toolboxEndpoint(base),order,component,request,preview};validate(record,base,order,component);
  const previous=readPendingToolbox(storage,base,order,component);
  if(previous&&JSON.stringify(previous)!==JSON.stringify(record))throw new Error("pending_toolbox_action_exists");
  storage.setItem(toolboxPendingKey(base,order,component),JSON.stringify(record));return record;
}
/** Call only after a matching acknowledgement or a confirmed rejection of that original request. */
export function clearPendingToolbox(storage:PendingStorage,record:PendingToolboxAction){
  const key=pendingKey(record.endpoint,record.order,record.component),raw=storage.getItem(key);if(raw===null)return;
  let current:PendingToolboxAction;try{current=JSON.parse(raw);}catch{throw new Error("pending_toolbox_journal_invalid");}
  if(current?.request?.idempotency_key!==record.request.idempotency_key||current?.preview?.request_sha256!==record.preview.request_sha256)throw new Error("pending_toolbox_action_exists");
  storage.removeItem(key);
}
function acknowledgedJob(value:unknown,record:PendingToolboxAction):ToolJob{
  const job=value as ToolJob&{idempotency_key:string,request_sha256:string};
  if(!job||typeof job.id!=="string"||!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(job.id)||job.order_id!==record.order||job.idempotency_key!==record.request.idempotency_key||job.request_sha256!==record.preview.request_sha256||!["running","completed","uncertain"].includes(job.state))throw new Error("pending_toolbox_acknowledgement_invalid");
  return job;
}
export async function recoverPendingToolbox(client:Client,record:PendingToolboxAction):Promise<ToolJob|null>{
  const jobs=await productRequest<(ToolJob&{idempotency_key:string,request_sha256:string})[]>(client,`/orders/${encodeURIComponent(record.order)}/toolbox/jobs`);
  const job=jobs.find(job=>job.idempotency_key===record.request.idempotency_key);
  if(job&&job.request_sha256!==record.preview.request_sha256)throw new Error("pending_toolbox_binding_changed");
  return job?acknowledgedJob(job,record):null;
}
export async function submitPendingToolbox(client:Client,record:PendingToolboxAction):Promise<ToolJob>{
  // Reconciliation is read-only. Existing admitted work is never submitted again with a new key.
  const recovered=await recoverPendingToolbox(client,record);if(recovered)return recovered;
  return acknowledgedJob(await productRequest<unknown>(client,`/orders/${encodeURIComponent(record.order)}/toolbox/jobs`,"POST",{...record.request,approval_sha256:record.preview.request_sha256}),record);
}
