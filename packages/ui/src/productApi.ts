import {Client,type KnowledgeProvenance,type Skill as ContractSkill} from "@cowork/client";
import type {MemoryMetadata} from "./memoryMetadata";

/** Extension services share transport/auth with the existing versioned API. */
export class ProductRequestError extends Error {
  readonly code:string;readonly status:number;readonly reason:string;
  constructor(code:string,status:number,reason=""){super(`${code} (${status})`);this.name="ProductRequestError";this.code=code;this.status=status;this.reason=reason;}
}
export async function productRequest<T>(client:Client,path:string,method="GET",body?:unknown,token=client.token):Promise<T> {
  const response=await fetch(client.base+"/api/v2"+path,{method,headers:{...(token?{Authorization:"Bearer "+token}:{}),...(body!==undefined?{"Content-Type":"application/json"}:{})},body:body===undefined?undefined:JSON.stringify(body),cache:"no-store"});
  if(!response.ok){let code="request_failed",reason="";try{const error=await response.json();code=String(error.code??error.error??code);if(typeof error.message==="string")reason=error.message.slice(0,2048);}catch{}throw new ProductRequestError(code,response.status,reason);}
  if(response.status===204)return undefined as T;
  return await response.json() as T;
}
export async function productDownload(client:Client,path:string):Promise<Blob>{
  const response=await fetch(client.base+"/api/v2"+path,{headers:{Authorization:"Bearer "+client.token},cache:"no-store"});
  if(!response.ok)throw new Error("download_failed ("+response.status+")");return response.blob();
}
export async function productUpload<T>(client:Client,path:string,bytes:Blob):Promise<T>{
  const response=await fetch(client.base+"/api/v2"+path,{method:"POST",headers:{Authorization:"Bearer "+client.token,"Content-Type":"application/zip"},body:bytes});
  if(!response.ok){let code="import_failed";try{const value=await response.json();code=String(value.code??value.error??code);}catch{}throw new Error(code+" ("+response.status+")");}return response.json() as Promise<T>;
}
export type Skill=ContractSkill;
export type Knowledge={id:string,project_id:string,kind:"knowledge"|"memory",title:string,content:string,tags:string[],revision:number,provenance?:KnowledgeProvenance,memory?:MemoryMetadata|null,folder_id?:string|null};
export type Schedule={id:string,project_id:string,name:string,timezone:string,recurrence:Record<string,string|number>,template:{order_id:string,prompt:string,provider_id:string,provider_revision:number,budgets:unknown},paused:boolean,next_run:number|null,active_occurrence:{run_id:string|null,uncertain:boolean}|null,last_run_id:string|null,last_error:string|null,revision:number};
export type ToolRequest={idempotency_key:string,action:Record<string,unknown>,timeout_seconds:number,output_bytes:number,approval_sha256?:string};
export type ToolPreview={request_sha256:string,mode:string,cwd:string,timeout_seconds:number,output_bytes:number,approval_required:boolean};
export type ToolJob={id:string,order_id:string,state:string,outcome:unknown,error:string|null};
