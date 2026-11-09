import type {Client} from "@cowork/client";

export type PrivateGithubBinding = {
  format:1; id:string; owner_user_id:string; project_id:string; revision:number;
  name:string; git_origin:string; api_root:string; owner:string; repository:string;
  author_name:string; author_email:string; binding_sha256:string;
};
export type PrivateGithubFields = Pick<PrivateGithubBinding,"name"|"git_origin"|"api_root"|"owner"|"repository"|"author_name"|"author_email"> & {expected_revision:number|null};
export type PrivateGithubProject = {id:string;name:string;revision:number};
export type PrivateGithubIntent = {format:1;operation_id:string;project_id:string;binding_id:string;remove:boolean;fields:PrivateGithubFields|{expected_revision:number}};
export const githubUuid=(v:unknown):v is string=>typeof v==="string"&&/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(v)&&v!=="00000000-0000-0000-0000-000000000000";

export class PrivateGithubError extends Error {
  constructor(public readonly code:string, public readonly status=0){super(code);}
}
function object(value:unknown):Record<string,unknown>{
  if(!value||typeof value!=="object"||Array.isArray(value))throw new PrivateGithubError("github_response_invalid");
  return value as Record<string,unknown>;
}
function keys(value:Record<string,unknown>,allowed:string[]){
  if(Object.keys(value).some(k=>!allowed.includes(k)))throw new PrivateGithubError("github_response_invalid");
}
export function githubBinding(value:unknown,actor:string,project:string):PrivateGithubBinding{
  const v=object(value);
  keys(v,["format","id","owner_user_id","project_id","revision","name","git_origin","api_root","owner","repository","author_name","author_email","binding_sha256"]);
  if(v.format!==1||!githubUuid(v.id)||v.owner_user_id!==actor||v.project_id!==project||!Number.isSafeInteger(v.revision)||Number(v.revision)<1
    ||!["name","git_origin","api_root","owner","repository","author_name","author_email"].every(k=>typeof v[k]==="string"&&String(v[k]).length>0&&String(v[k]).length<=2048)
    ||typeof v.binding_sha256!=="string"||!/^[0-9a-f]{64}$/.test(v.binding_sha256))throw new PrivateGithubError("github_response_invalid");
  return v as PrivateGithubBinding;
}
export function githubIntent(value:unknown):PrivateGithubIntent{
  const v=object(value);keys(v,["format","operation_id","project_id","binding_id","remove","fields"]);
  if(v.format!==1||!githubUuid(v.operation_id)||!githubUuid(v.project_id)||!githubUuid(v.binding_id)||typeof v.remove!=="boolean")throw new PrivateGithubError("github_original_invalid");
  const f=object(v.fields);
  const expected=f.expected_revision;
  if(!(expected===null||Number.isSafeInteger(expected)&&Number(expected)>0)||v.remove&&expected===null)throw new PrivateGithubError("github_original_invalid");
  keys(f,v.remove?["expected_revision"]:["expected_revision","name","git_origin","api_root","owner","repository","author_name","author_email"]);
  if(!v.remove&&!["name","git_origin","api_root","owner","repository","author_name","author_email"].every(k=>typeof f[k]==="string"&&String(f[k]).length>0&&String(f[k]).length<=2048))throw new PrivateGithubError("github_original_invalid");
  return v as PrivateGithubIntent;
}
async function request(client:Client,session:string,path:string,method="GET",body?:unknown,key?:string):Promise<unknown>{
  if(!session)throw new PrivateGithubError("github_session_required");
  const controller=new AbortController();const timer=setTimeout(()=>controller.abort(),30000);
  try{
    const response=await fetch(client.base+(path.startsWith("/api/v2/")?path:"/api/v1"+path),{method,credentials:"omit",redirect:"error",cache:"no-store",signal:controller.signal,
      headers:{Authorization:"Bearer "+session,...(body===undefined?{}:{"Content-Type":"application/json"}),...(key?{"Idempotency-Key":key}:{})},
      body:body===undefined?undefined:JSON.stringify(body)});
    if(!response.ok){let code="github_request_failed";try{const v=object(await response.json());if(typeof (v.code??v.error)==="string")code=String(v.code??v.error);}catch{}
      throw new PrivateGithubError(code,response.status);
    }
    return await response.json();
  }catch(e){if(e instanceof PrivateGithubError)throw e;throw new PrivateGithubError(method==="GET"?"github_connection_failed":"github_original_unconfirmed");}
  finally{clearTimeout(timer);}
}
export async function privateGithubProjects(client:Client,session:string):Promise<PrivateGithubProject[]>{
  const raw=await request(client,session,"/api/v2/actor-model-registry/projects");
  if(!Array.isArray(raw)||raw.length>1000)throw new PrivateGithubError("github_response_invalid");
  return raw.map(v=>{const x=object(v);if(!githubUuid(x.id)||typeof x.name!=="string"||!Number.isSafeInteger(x.revision)||Number(x.revision)<1)throw new PrivateGithubError("github_response_invalid");return {id:x.id,name:x.name,revision:Number(x.revision)};});
}
export async function privateGithubBindings(client:Client,session:string,actor:string,project:string):Promise<PrivateGithubBinding[]>{
  if(!githubUuid(project)||!githubUuid(actor))throw new PrivateGithubError("github_context_invalid");
  const raw=await request(client,session,`/projects/${project}/github-bindings`);
  if(!Array.isArray(raw)||raw.length>128)throw new PrivateGithubError("github_response_invalid");
  return raw.map(v=>githubBinding(v,actor,project));
}
export async function privateGithubMutate(client:Client,session:string,actor:string,intent:PrivateGithubIntent,token?:string):Promise<PrivateGithubBinding|{id:string;revision:number;deleted:true}>{
  const plan=githubIntent(intent);
  const path=`/projects/${plan.project_id}/github-bindings/${plan.binding_id}`;
  const raw=await request(client,session,plan.remove?path+`?expected_revision=${plan.fields.expected_revision}`:path,plan.remove?"DELETE":"PUT",plan.remove?undefined:{...plan.fields,token},plan.operation_id);
  if(plan.remove){const v=object(raw);keys(v,["id","revision","deleted"]);if(v.id!==plan.binding_id||v.revision!==Number(plan.fields.expected_revision)+1||v.deleted!==true)throw new PrivateGithubError("github_response_invalid");return v as {id:string;revision:number;deleted:true};}
  const value=githubBinding(raw,actor,plan.project_id);
  if(value.id!==plan.binding_id||value.revision!==(plan.fields.expected_revision??0)+1)throw new PrivateGithubError("github_response_invalid");
  return value;
}
