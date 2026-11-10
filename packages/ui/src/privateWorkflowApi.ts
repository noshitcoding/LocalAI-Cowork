import {decode,type AndroidWorkflowEntry,type AndroidWorkflowEngine,type AndroidWorkflowVersionRef,type AndroidWorkflowWrite,type AndroidWorkflowChange} from "@cowork/client";
import {uuid,sha256} from "./androidProjectVersionsApi";

export type PrivateWorkflowConfig={runner:AndroidWorkflowEngine;workflow_id?:string;crew_id?:string;workflow_version_id:string};
export const workflowConfig=(entry:AndroidWorkflowEntry,version:AndroidWorkflowVersionRef):PrivateWorkflowConfig=>({runner:entry.engine,...(entry.engine==="crew"?{crew_id:entry.id}:{workflow_id:entry.id}),workflow_version_id:version.id});
export function canonicalJson(value:unknown):string{if(value===null||typeof value!=="object")return JSON.stringify(value);if(Array.isArray(value))return "["+value.map(canonicalJson).join(",")+"]";const object=value as Record<string,unknown>;return "{"+Object.keys(object).sort().map(key=>JSON.stringify(key)+":"+canonicalJson(object[key])).join(",")+"}";}
export const workflowHash=(value:unknown)=>sha256(new TextEncoder().encode(canonicalJson(value)));
export class PrivateWorkflowRequestError extends Error{constructor(public readonly code:string,public readonly status:number){super(code);}}
export async function privateWorkflowRequest<T>(endpoint:string,token:string,path:string,method="GET",body?:unknown):Promise<T>{
 const response=await fetch(endpoint+"/api/v1"+path,{method,headers:{Authorization:"Bearer "+token,...(body===undefined?{}:{"Content-Type":"application/json"})},body:body===undefined?undefined:JSON.stringify(body),credentials:"omit",cache:"no-store",redirect:"error"});
 if(!response.ok){let code="private_workflow_request_failed";try{code=String((await response.json()).error??code);}catch{}throw new PrivateWorkflowRequestError(code,response.status);}
 const reader=response.body?.getReader();if(!reader)throw new Error("private_workflow_original_invalid");let size=0,raw="";const decoder=new TextDecoder("utf-8",{fatal:true});
 try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>3*1024*1024)throw new Error("private_workflow_response_bound");raw+=decoder.decode(part.value,{stream:true});}raw+=decoder.decode();return JSON.parse(raw) as T;}finally{await reader.cancel().catch(()=>{});reader.releaseLock();}
}
export function workflowEntry(value:unknown,actor:string,project:string){const entry=decode("AndroidWorkflowEntry",value);if(entry.owner_id!==actor||entry.project_id!==project)throw new Error("private_workflow_context_changed");return entry;}
export function workflowDefinition(value:unknown,engine:AndroidWorkflowEngine):unknown{if(new TextEncoder().encode(JSON.stringify(value)).length>2*1024*1024)throw new Error("private_workflow_definition_bound");if(engine==="workflow")return decode("WorkflowDefinition",value);if(!value||typeof value!=="object"||Array.isArray(value))throw new Error("private_workflow_original_invalid");return value;}
export type WorkflowOriginal={format:1;endpoint:string;actor:string;project:string;key:string;path:string;metadata:Omit<AndroidWorkflowWrite,"definition">|AndroidWorkflowChange;definition_sha256:string|null;request_sha256:string};
export function restoreWorkflowOriginal(raw:string,endpoint:string,actor:string,project:string):WorkflowOriginal{
 if(raw.length>32768)throw new Error("private_workflow_original_invalid");const value=JSON.parse(raw) as WorkflowOriginal;
 if(value?.format!==1||value.endpoint!==endpoint||value.actor!==actor||value.project!==project||!uuid(value.key)||value.metadata?.request_id!==value.key||Object.hasOwn(value.metadata,"definition")||!/^[0-9a-f]{64}$/.test(value.request_sha256)||value.definition_sha256!==null&&!/^[0-9a-f]{64}$/.test(value.definition_sha256)||Object.keys(value).some(key=>!["format","endpoint","actor","project","key","path","metadata","definition_sha256","request_sha256"].includes(key)))throw new Error("private_workflow_original_invalid");
 const parts=value.path.split("?")[0].split("/"),id=parts[2];if(id!==undefined&&!uuid(id))throw new Error("private_workflow_original_invalid");
 if(value.definition_sha256===null){if(value.path!==`/private-workflows/${id}/publication?project_id=${project}`)throw new Error("private_workflow_original_invalid");decode("AndroidWorkflowChange",value.metadata);}
 else{if(value.path!=="/private-workflows"&&value.path!==`/private-workflows/${id}/versions`)throw new Error("private_workflow_original_invalid");const body=decode("AndroidWorkflowWrite",{...value.metadata,definition:null});if(body.project_id!==project)throw new Error("private_workflow_original_invalid");}
 return value;
}
export function workflowOperation(path:string,body:unknown){const id=path.split("?")[0].split("/")[2];return {format:"android-private-workflow-operation/1",workflow_id:id??null,request:body};}
