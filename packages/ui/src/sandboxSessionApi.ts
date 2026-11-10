import type {Client} from "@cowork/client";
import type {EnvironmentReference} from "./EnvironmentPanel";
import {PersistentRequestLatch,type RequestJournalStorage} from "./requestJournal";
import {productRequest} from "./productApi";
export type SandboxOperation="ensure"|"inspect"|"suspend"|"resume"|"stop";
export type SandboxSessionRequest={environment:EnvironmentReference,idempotency_key:string,operation:SandboxOperation,expected_sandbox_uid:string|null};
export type SandboxSessionReceipt={project_id:string,order_id:string,environment:EnvironmentReference,operation_id:string,sandbox_name:string,sandbox_uid:string|null,pod_uid:string|null,state:"provisioning"|"ready"|"suspended"|"stopping"|"stopped"|"unknown",workspace_preserved:boolean,observed_at:number};
const operations:SandboxOperation[]=["ensure","inspect","suspend","resume","stop"];
const binding=(a:EnvironmentReference,b:EnvironmentReference)=>["environment_id","version_id","definition_sha256","qualification_sha256"].every(name=>a[name as keyof EnvironmentReference]===b[name as keyof EnvironmentReference]);
export function validateSandboxRequest(raw:unknown):SandboxSessionRequest {
  const value=raw as SandboxSessionRequest;
  if(!value||Object.keys(value).some(key=>!["environment","idempotency_key","operation","expected_sandbox_uid"].includes(key))||!operations.includes(value.operation)||typeof value.idempotency_key!=="string"||!value.idempotency_key||value.idempotency_key.length>128||!(value.expected_sandbox_uid===null||typeof value.expected_sandbox_uid==="string"&&value.expected_sandbox_uid.length>0))throw new Error("sandbox_request_invalid");
  const environment=value.environment;
  if(!environment||Object.keys(environment).length!==4||![environment.environment_id,environment.version_id].every(v=>typeof v==="string"&&v.length>0)||![environment.definition_sha256,environment.qualification_sha256].every(v=>typeof v==="string"&&/^[a-f0-9]{64}$/.test(v)))throw new Error("sandbox_environment_invalid");
  return structuredClone(value);
}
export class SandboxSessionLatch extends PersistentRequestLatch<SandboxSessionRequest>{constructor(key?:string,storage?:RequestJournalStorage){super(key,validateSandboxRequest,storage);}}
export function validateSandboxReceipt(raw:unknown,project:string,order:string,request:SandboxSessionRequest):SandboxSessionReceipt {
  const value=raw as SandboxSessionReceipt;
  if(!value||value.project_id!==project||value.order_id!==order||!value.environment||!binding(value.environment,request.environment)||typeof value.operation_id!=="string"||!value.operation_id||typeof value.sandbox_name!=="string"||!value.sandbox_name||!["provisioning","ready","suspended","stopping","stopped","unknown"].includes(value.state)||typeof value.workspace_preserved!=="boolean"||!Number.isSafeInteger(value.observed_at)||![value.sandbox_uid,value.pod_uid].every(v=>v===null||typeof v==="string"&&v.length>0)||request.operation!=="ensure"&&request.expected_sandbox_uid&&value.sandbox_uid&&value.sandbox_uid!==request.expected_sandbox_uid)throw new Error("sandbox_session_receipt_mismatch");
  return value;
}
export class SandboxSessionRequestError extends Error {constructor(readonly code:string,readonly status:number){super(`${code} (${status})`);}}
export async function manageSandboxSession(client:Client,project:string,order:string,input:SandboxSessionRequest):Promise<SandboxSessionReceipt>{
  const request=validateSandboxRequest(input);
  const response=await fetch(client.base+`/api/v2/projects/${encodeURIComponent(project)}/orders/${encodeURIComponent(order)}/sandbox-session`,{method:"POST",headers:{...(client.token?{Authorization:"Bearer "+client.token}:{}),"Content-Type":"application/json"},body:JSON.stringify(request),cache:"no-store"});
  if(!response.ok){let code="sandbox_request_failed";try{const value=await response.json();code=String(value.code??value.error??code);}catch{}throw new SandboxSessionRequestError(code,response.status);}
  return validateSandboxReceipt(await response.json(),project,order,request);
}
/** A separate observation never replays or settles a saved mutating request. */
export async function inspectSandboxSession(client:Client,project:string,order:string,environment:EnvironmentReference):Promise<SandboxSessionReceipt|null>{
  try{return await manageSandboxSession(client,project,order,{environment:structuredClone(environment),idempotency_key:crypto.randomUUID(),operation:"inspect",expected_sandbox_uid:null});}
  catch(error){if(error instanceof SandboxSessionRequestError&&error.status===404&&error.code==="sandbox_session_not_found")return null;throw error;}
}
/** Existing sessions require their observed physical identity before any mutation. */
export function sandboxExpectedUid(operation:SandboxOperation,session:SandboxSessionReceipt|null,observed:boolean):string|null {
  if(operation==="inspect")return null;
  if(!observed)throw new Error("sandbox_session_inspection_required");
  if(operation==="ensure"&&(!session||session.state==="stopped"))return null;
  if(!session?.sandbox_uid)throw new Error("sandbox_session_not_found");
  return session.sandbox_uid;
}
export function validateSandboxPreview(value:unknown,appOrigin:string,now=Date.now()):{url:string,expires_at:number}{
  const receipt=value as {url:string,expires_at:number};
  if(!receipt||typeof receipt.url!=="string"||!Number.isSafeInteger(receipt.expires_at)||receipt.expires_at<=now||receipt.expires_at>now+60_000)throw new Error("sandbox_preview_receipt_invalid");
  const url=new URL(receipt.url),app=new URL(appOrigin);
  if(url.protocol!=="https:"||url.hostname===app.hostname||url.username||url.password||url.search||url.pathname!=="/_cowork/bootstrap"||!/^#[A-Za-z0-9_-]{1,3500}$/.test(url.hash))throw new Error("sandbox_preview_origin_invalid");
  return receipt;
}
export async function createSandboxPreview(client:Client,project:string,order:string,environment:EnvironmentReference,uid:string,appOrigin:string){
  const receipt=await productRequest(client,`/projects/${encodeURIComponent(project)}/orders/${encodeURIComponent(order)}/sandbox-preview`,"POST",{environment,expected_sandbox_uid:uid,port:8081});
  return validateSandboxPreview(receipt,appOrigin);
}
export type SandboxOperationRecord={id:string,state:string,request:SandboxSessionRequest};
export async function sandboxOperationHistory(client:Client,project:string,order:string):Promise<SandboxOperationRecord[]>{
  const values=await productRequest<unknown>(client,`/projects/${encodeURIComponent(project)}/orders/${encodeURIComponent(order)}/sandbox-operations`);
  if(!Array.isArray(values))throw new Error("sandbox_operation_history_invalid");
  return values.map(raw=>{const value=raw as {id:string,state:string,request:unknown};if(!value||typeof value.id!=="string"||!value.id||typeof value.state!=="string")throw new Error("sandbox_operation_history_invalid");return {...value,request:validateSandboxRequest(value.request)};});
}
export function unresolvedSandboxOperation(values:SandboxOperationRecord[],environment:EnvironmentReference):SandboxSessionRequest|null{return values.find(value=>["pending","dispatched"].includes(value.state)&&binding(value.request.environment,environment))?.request??null;}
export async function recoverSandboxOperation(client:Client,project:string,order:string,environment:EnvironmentReference):Promise<SandboxSessionRequest|null>{return unresolvedSandboxOperation(await sandboxOperationHistory(client,project,order),environment);}
export function definitiveSandboxRejection(values:SandboxOperationRecord[],original:SandboxSessionRequest):boolean{
  const value=values.find(value=>value.request.idempotency_key===original.idempotency_key);if(!value)return false;
  if(!binding(value.request.environment,original.environment)||value.request.operation!==original.operation||value.request.expected_sandbox_uid!==original.expected_sandbox_uid)throw new Error("sandbox_original_request_changed");
  return ["rejected","expired"].includes(value.state);
}
