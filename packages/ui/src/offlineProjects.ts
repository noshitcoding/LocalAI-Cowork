import type {Client} from "@cowork/client";
import {replicaDatabase} from "./teamReplicaDatabase";
import {emptyReplica,replicaScope,synchronizeReplica,validateReplicaCapacity,type TeamReplica,type ReplicaEntity} from "./teamReplica";
import {handoffProjectFiles,synchronizeFiles,type ProjectFileHandoff} from "./teamReplicaFiles";

export type OfflineWorkspace={scope:string,endpoint:string,identity:string,team:string,name:string,can_create:boolean,observed_at:number};
export type TenantProjectReceipt={id:string,organization_id:string,name:string,revision:number,created_at:number};
export type OfflineProject={scope:string,workspace:string,endpoint:string,identity:string,team:string,local_id:string,request:{name:string,idempotency_key:string},request_path:string,request_json:string,request_sha256:string,created_at:number,state:"pending"|"uncertain"|"mapped"|"cancelled",receipt:TenantProjectReceipt|null,receipt_sha256:string|null,file_handoff:"pending"|"complete",last_error?:string};
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const nil="00000000-0000-0000-0000-000000000000",prefix="local-project:",maximumProjects=1000;
function fail(code:string):never{throw new Error("offline_project_"+code);}
function identifiers(endpoint:string,identity:string,team:string){
 if(typeof endpoint!=="string"||typeof identity!=="string"||typeof team!=="string")fail("scope");
 // Client.base === "" means this web origin. IndexedDB already belongs to that
 // origin; retain the exact endpoint in every scope and request binding rather
 // than silently remapping existing local contents to an absolute server.
 if(endpoint!==""){
  let url:URL;try{url=new URL(endpoint);}catch{fail("scope");}
  if(!["http:","https:"].includes(url.protocol)||url.username||url.password||url.search||url.hash)fail("scope");
 }
 if(!uuid.test(identity)||identity===nil||!uuid.test(team)||team===nil)fail("scope");
}
function name(value:string){if(typeof value!=="string"||!value.trim()||[...value.trim()].length>200||/[\x00-\x1f\x7f-\x9f]/.test(value))fail("name");return value.trim();}
export const offlineWorkspaceScope=(endpoint:string,identity:string,team:string)=>JSON.stringify([endpoint,identity,team]);
async function digest(value:string){return [...new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value)))].map(n=>n.toString(16).padStart(2,"0")).join("");}
const creationPath=(team:string)=>`/tenant/organizations/${team}/projects`;
const requestBinding=(row:Pick<OfflineProject,"endpoint"|"identity"|"team"|"request_path"|"request_json">)=>JSON.stringify(["offline_project_create/v1",row.endpoint,row.identity,row.team,row.request_path,row.request_json]);
function validate(row:OfflineProject){identifiers(row.endpoint,row.identity,row.team);if(!row.local_id.startsWith(prefix)||!uuid.test(row.local_id.slice(prefix.length))||row.scope!==replicaScope(row.endpoint,row.identity,row.team,row.local_id)||row.workspace!==offlineWorkspaceScope(row.endpoint,row.identity,row.team)||!uuid.test(row.request.idempotency_key)||row.request.idempotency_key===nil||row.request.name!==name(row.request.name)||row.request_path!==creationPath(row.team)||row.request_json!==JSON.stringify({name:row.request.name,idempotency_key:row.request.idempotency_key})||!/^[0-9a-f]{64}$/.test(row.request_sha256)||!["pending","uncertain","mapped","cancelled"].includes(row.state)||!["pending","complete"].includes(row.file_handoff)||!Number.isSafeInteger(row.created_at)||row.created_at<=0)fail("integrity");if(row.receipt){validateReceipt(row,row.receipt);if(row.state!=="mapped"||!row.receipt_sha256||!/^[0-9a-f]{64}$/.test(row.receipt_sha256))fail("mapping");}else if(row.state==="mapped"||row.file_handoff==="complete"||row.receipt_sha256)fail("mapping");}
function validateReceipt(row:OfflineProject,receipt:TenantProjectReceipt){if(!receipt||Object.keys(receipt).sort().join(",")!=="created_at,id,name,organization_id,revision"||typeof receipt.id!=="string"||!uuid.test(receipt.id)||receipt.id===nil||receipt.organization_id!==row.team||receipt.name!==row.request.name||receipt.revision!==1||!Number.isSafeInteger(receipt.created_at)||receipt.created_at<=0)fail("receipt");}
const receiptBytes=(value:TenantProjectReceipt)=>JSON.stringify({id:value.id,organization_id:value.organization_id,name:value.name,revision:value.revision,created_at:value.created_at});
export async function offlineWorkspaces(endpoint:string,identity?:string):Promise<OfflineWorkspace[]>{const db=await replicaDatabase();try{return await new Promise((resolve,reject)=>{const r=db.transaction("workspaces").objectStore("workspaces").getAll();r.onsuccess=()=>resolve((r.result as OfflineWorkspace[]).filter(w=>w.endpoint===endpoint&&(!identity||w.identity===identity)).sort((a,b)=>b.observed_at-a.observed_at));r.onerror=()=>reject(r.error);});}finally{db.close();}}
/** Only public workspace metadata is cached. Tokens/passwords/provider keys
 * never enter either IndexedDB, and this snapshot grants no server authority. */
export async function rememberOfflineWorkspace(value:Omit<OfflineWorkspace,"scope"|"observed_at">):Promise<void>{identifiers(value.endpoint,value.identity,value.team);if(typeof value.can_create!=="boolean")fail("workspace");const row:OfflineWorkspace={endpoint:value.endpoint,identity:value.identity,team:value.team,can_create:value.can_create,name:name(value.name),scope:offlineWorkspaceScope(value.endpoint,value.identity,value.team),observed_at:Date.now()};const db=await replicaDatabase();try{await new Promise<void>((resolve,reject)=>{const tx=db.transaction("workspaces","readwrite");tx.objectStore("workspaces").put(row);tx.oncomplete=()=>resolve();tx.onabort=tx.onerror=()=>reject(tx.error);});}finally{db.close();}}
export async function offlineProjects(workspace:string):Promise<OfflineProject[]>{const db=await replicaDatabase();try{return await new Promise((resolve,reject)=>{const r=db.transaction("project_creations").objectStore("project_creations").getAll();r.onsuccess=()=>{try{const values=(r.result as OfflineProject[]).filter(p=>p.workspace===workspace);values.forEach(validate);resolve(values.sort((a,b)=>b.created_at-a.created_at));}catch(error){reject(error);}};r.onerror=()=>reject(r.error);});}finally{db.close();}}
export async function readOfflineProject(scope:string):Promise<OfflineProject>{const db=await replicaDatabase();try{return await new Promise((resolve,reject)=>{const r=db.transaction("project_creations").objectStore("project_creations").get(scope);r.onsuccess=()=>{try{if(!r.result)fail("missing");validate(r.result);resolve(r.result);}catch(error){reject(error);}};r.onerror=()=>reject(r.error);});}finally{db.close();}}
async function updateProject(scope:string,change:(row:OfflineProject)=>void):Promise<OfflineProject>{const db=await replicaDatabase();try{return await new Promise((resolve,reject)=>{const tx=db.transaction("project_creations","readwrite"),store=tx.objectStore("project_creations"),r=store.get(scope);let result:OfflineProject;r.onsuccess=()=>{try{if(!r.result)fail("missing");result=r.result;validate(result);change(result);validate(result);store.put(result);}catch(error){reject(error);tx.abort();}};tx.oncomplete=()=>resolve(result);tx.onabort=tx.onerror=()=>reject(tx.error??new Error("offline_project_storage"));});}finally{db.close();}}
export async function queueOfflineProject(workspace:OfflineWorkspace,projectName:string):Promise<OfflineProject>{
 identifiers(workspace.endpoint,workspace.identity,workspace.team);if(workspace.scope!==offlineWorkspaceScope(workspace.endpoint,workspace.identity,workspace.team)||!workspace.can_create)fail("workspace");
 const request={name:name(projectName),idempotency_key:crypto.randomUUID()},local_id=prefix+crypto.randomUUID(),scope=replicaScope(workspace.endpoint,workspace.identity,workspace.team,local_id);
 const row:OfflineProject={scope,workspace:workspace.scope,endpoint:workspace.endpoint,identity:workspace.identity,team:workspace.team,local_id,request,request_path:creationPath(workspace.team),request_json:JSON.stringify(request),request_sha256:"",created_at:Date.now(),state:"pending",receipt:null,receipt_sha256:null,file_handoff:"pending"};row.request_sha256=await digest(requestBinding(row));validate(row);
 const replica=emptyReplica(scope);replica.entities["project:"+local_id]={id:local_id,team_id:row.team,name:request.name,archived:false,revision:0,created_at:row.created_at};
 const db=await replicaDatabase();try{await new Promise<void>((resolve,reject)=>{const tx=db.transaction(["project_creations","replicas","workspaces"],"readwrite"),projects=tx.objectStore("project_creations"),count=projects.count(),known=tx.objectStore("workspaces").get(workspace.scope);let ready=2;const save=()=>{if(--ready)return;try{if(count.result>=maximumProjects)fail("capacity");if(!known.result||JSON.stringify(known.result)!==JSON.stringify(workspace)||!known.result.can_create)fail("workspace");projects.add(row);tx.objectStore("replicas").add(replica);}catch(error){reject(error);tx.abort();}};count.onsuccess=save;known.onsuccess=save;tx.oncomplete=()=>resolve();tx.onabort=tx.onerror=()=>reject(tx.error??new Error("offline_project_storage"));});}finally{db.close();}return row;
}
export async function setOfflineProjectCancelled(scope:string,cancelled:boolean):Promise<OfflineProject>{return updateProject(scope,row=>{if(cancelled&&row.state!=="pending"||!cancelled&&row.state!=="cancelled")fail("uncertain");row.state=cancelled?"cancelled":"pending";});}
function sameIntent(left:OfflineProject,right:OfflineProject){return left.scope===right.scope&&left.request_path===right.request_path&&left.request_json===right.request_json&&left.request_sha256===right.request_sha256&&left.workspace===right.workspace&&left.local_id===right.local_id;}
/** The original receipt, immutable mapping and all content remaps commit in one
 * IDB transaction. File bytes belong to a different DB and are journaled next. */
export async function acknowledgeOfflineProject(original:OfflineProject,receipt:TenantProjectReceipt):Promise<OfflineProject>{
 validate(original);validateReceipt(original,receipt);if(await digest(requestBinding(original))!==original.request_sha256)fail("integrity");const receipt_sha256=await digest(receiptBytes(receipt)),targetScope=replicaScope(original.endpoint,original.identity,original.team,receipt.id),db=await replicaDatabase();
 try{return await new Promise((resolve,reject)=>{const tx=db.transaction(["project_creations","replicas"],"readwrite"),projects=tx.objectStore("project_creations"),replicas=tx.objectStore("replicas"),row=projects.get(original.scope),source=replicas.get(original.scope),target=replicas.get(targetScope);let pending=3,result:OfflineProject;
  const ready=()=>{if(--pending)return;try{
   if(!row.result||!sameIntent(row.result,original))fail("integrity");result=row.result;validate(result);
   if(result.receipt){if(receiptBytes(result.receipt)!==receiptBytes(receipt)||result.receipt_sha256!==receipt_sha256)fail("mapping");return;}
   if(result.state!=="uncertain"||!source.result||source.result.mapped_to||target.result?.mapped_to)fail("mapping");
   const from:TeamReplica=source.result,to:TeamReplica=target.result??emptyReplica(targetScope);if(from.scope!==original.scope||from.cursor!==0||to.scope!==targetScope||from.pending.some(p=>p.state!=="pending")||to.project_mapping&&(to.project_mapping.local_id!==original.local_id||to.project_mapping.server_id!==receipt.id))fail("mapping");
   const serverProject:ReplicaEntity={id:receipt.id,team_id:original.team,name:receipt.name,revision:1,archived:false,created_at:receipt.created_at};
   const remap=(value:ReplicaEntity|null):ReplicaEntity|null=>{if(!value)return null;if(value.team_id!==undefined&&value.team_id!==original.team)fail("scope");return{...value,...(value.id===original.local_id?{id:receipt.id,revision:1}:{}),...(value.project_id===original.local_id?{project_id:receipt.id}:{})};};
   for(const [key,value] of Object.entries(from.entities)){if(key==="project:"+original.local_id)continue;if(to.entities[key]&&JSON.stringify(to.entities[key])!==JSON.stringify(value))fail("collision");to.entities[key]=remap(value)!;}
   for(const pending of from.pending){const next=structuredClone(pending);if(next.request.kind==="project"){
     if(next.request.entity_id!==original.local_id||next.request.expected_revision!==0)fail("mapping");next.request.entity_id=receipt.id;next.request.expected_revision=1;next.request.value.expected_revision=1;next.base=serverProject;
    }else{next.base=remap(next.base);if(next.remote!==undefined)next.remote=remap(next.remote);if(next.request.value.project_id===original.local_id)next.request.value.project_id=receipt.id;}
    if(to.pending.some(p=>p.request.idempotency_key===next.request.idempotency_key||p.request.kind===next.request.kind&&p.request.entity_id===next.request.entity_id))fail("collision");to.pending.push(next);
   }
   const localProject=to.pending.find(p=>p.request.kind==="project"&&p.request.entity_id===receipt.id);to.entities["project:"+receipt.id]=localProject?{...serverProject,...localProject.request.value,id:receipt.id,revision:1}:to.entities["project:"+receipt.id]??serverProject;
   to.project_mapping={local_id:original.local_id,server_id:receipt.id};validateReplicaCapacity(to);
   result={...result,state:"mapped",receipt:JSON.parse(receiptBytes(receipt)) as TenantProjectReceipt,receipt_sha256,file_handoff:"pending"};validate(result);
   projects.put(result);replicas.put(to);replicas.put({...emptyReplica(original.scope),mapped_to:targetScope,updated_at:Date.now()});
  }catch(error){reject(error);tx.abort();}};row.onsuccess=source.onsuccess=target.onsuccess=ready;tx.oncomplete=()=>resolve(result);tx.onabort=tx.onerror=()=>reject(tx.error??new Error("offline_project_storage"));
 });}finally{db.close();}
}
export async function finishOfflineProjectFiles(scope:string):Promise<OfflineProject>{
 const row=await readOfflineProject(scope);if(!row.receipt||!row.receipt_sha256)fail("mapping");if(await digest(receiptBytes(row.receipt))!==row.receipt_sha256||await digest(requestBinding(row))!==row.request_sha256)fail("integrity");
 const handoff:ProjectFileHandoff={source_scope:row.scope,target_scope:replicaScope(row.endpoint,row.identity,row.team,row.receipt.id),creation_key:row.request.idempotency_key,request_sha256:row.request_sha256,receipt_sha256:row.receipt_sha256,state:"committed",completed_at:0};
 await handoffProjectFiles(handoff);
 return updateProject(scope,current=>{if(!sameIntent(current,row)||!current.receipt||receiptBytes(current.receipt)!==receiptBytes(row.receipt!))fail("mapping");current.file_handoff="complete";});
}
class ProjectHttpError extends Error{constructor(readonly status:number,readonly code:string){super(code+" ("+status+")");}}
async function request<T>(client:Client,session:string,path:string,body?:string):Promise<T>{const cancel=new AbortController(),timer=setTimeout(()=>cancel.abort(),30_000);try{
 const response=await fetch(client.base+"/api/v2"+path,{method:body===undefined?"GET":"POST",headers:{Authorization:"Bearer "+session,...(body===undefined?{}:{"Content-Type":"application/json"})},body,cache:"no-store",credentials:"omit",redirect:"error",signal:cancel.signal});
 const reader=response.body?.getReader();if(!reader)fail("response");const chunks:Uint8Array[]=[],limit=8*1024;let length=0;try{for(;;){const part=await reader.read();if(part.done)break;length+=part.value.length;if(length>limit){await reader.cancel();fail("response");}chunks.push(part.value);}}finally{reader.releaseLock();}
 const bytes=new Uint8Array(length);let offset=0;for(const chunk of chunks){bytes.set(chunk,offset);offset+=chunk.length;}let value:unknown;try{value=JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(bytes));}catch{fail("response");}
 if(!response.ok){const code=(value as {error?:unknown,code?:unknown})?.error??(value as {code?:unknown})?.code;throw new ProjectHttpError(response.status,typeof code==="string"&&/^[a-z0-9_]{1,80}$/.test(code)?code:"offline_project_server");}return value as T;
 }catch(error){if(error instanceof TypeError||error instanceof DOMException&&error.name==="AbortError")fail("unconfirmed");throw error;}finally{clearTimeout(timer);}}
/** Explicit synchronization only. Unknown outcomes retry exactly the original
 * authenticated tenant request, never the latest local name or a client ID. */
export async function synchronizeOfflineProject(client:Client,session:string,identity:string,scope:string):Promise<OfflineProject>{
 let row=await readOfflineProject(scope);if(row.endpoint!==client.base||row.identity!==identity||!session)fail("identity");if(row.state==="cancelled")fail("cancelled");
 const actor=await request<{id:string}>(client,session,"/identity/me");if(actor.id!==row.identity)fail("identity");if(await digest(requestBinding(row))!==row.request_sha256)fail("integrity");
 if(!row.receipt){row=await updateProject(scope,current=>{if(!sameIntent(current,row))fail("integrity");if(current.state==="pending"||current.state==="uncertain")current.state="uncertain";else if(current.state!=="mapped")fail("cancelled");delete current.last_error;});
  if(!row.receipt)try{const receipt=await request<TenantProjectReceipt>(client,session,row.request_path,row.request_json);row=await acknowledgeOfflineProject(row,receipt);}catch(error){await updateProject(scope,current=>{if(!current.receipt)current.last_error=error instanceof ProjectHttpError?error.code:"offline_project_unconfirmed";});throw error;}
 }
 row=await finishOfflineProjectFiles(scope);const project=row.receipt!.id,targetScope=replicaScope(row.endpoint,row.identity,row.team,project);
 await synchronizeReplica(client,session,targetScope,row.team,project,true);await synchronizeFiles(client,session,targetScope,row.team,project);await synchronizeReplica(client,session,targetScope,row.team,project);
 return readOfflineProject(scope);
}
