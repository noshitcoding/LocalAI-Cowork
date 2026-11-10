import {Client} from "@cowork/client";
import {productRequest} from "./productApi";
import {replicaDatabase as database} from "./teamReplicaDatabase";
export type ReplicaEntity={id:string,revision?:number,[key:string]:unknown};
export type ReplicaMutation={idempotency_key:string,kind:string,entity_id:string,expected_revision:number,value:Record<string,unknown>};
export type ReplicaPending={request:ReplicaMutation,base:ReplicaEntity|null,state:"pending"|"uncertain"|"conflict",remote?:ReplicaEntity|null};
export type TeamReplica={scope:string,cursor:number,entities:Record<string,ReplicaEntity>,pending:ReplicaPending[],updated_at:number,mapped_to?:string,project_mapping?:{local_id:string,server_id:string}};
type Change={cursor:number,kind:string,entity_id:string,payload:ReplicaEntity,tombstone:boolean};
type Page={changes:Change[],next_cursor:number,watermark_cursor:number,has_more:boolean};
export const replicaScope=(endpoint:string,identity:string,team:string,project:string)=>JSON.stringify([endpoint,identity,team,project]);
export const emptyReplica=(scope:string):TeamReplica=>({scope,cursor:0,entities:{},pending:[],updated_at:0});
export function validateReplicaCapacity(value:TeamReplica){if(value.pending.length>100||new TextEncoder().encode(JSON.stringify(value)).length>32*1024*1024)throw new Error("replica_capacity");}
/** Alias lookup and its destination stay in the same IDB transaction. */
function replicaRecord(store:IDBObjectStore,scope:string,receive:(value:TeamReplica)=>void,fail:(error:unknown)=>void){const request=store.get(scope);request.onerror=()=>fail(request.error);request.onsuccess=()=>{const value:TeamReplica=request.result??emptyReplica(scope);if(value.scope!==scope){fail(new Error("offline_project_scope"));return;}if(value.mapped_to){const target=store.get(value.mapped_to);target.onerror=()=>fail(target.error);target.onsuccess=()=>{if(!target.result||target.result.scope!==value.mapped_to||target.result.mapped_to){fail(new Error("offline_project_mapping"));return;}receive(target.result);};}else receive(value);};}
export async function readReplica(scope:string):Promise<TeamReplica>{const db=await database();try{return await new Promise((resolve,reject)=>{replicaRecord(db.transaction("replicas").objectStore("replicas"),scope,resolve,reject);});}finally{db.close();}}
async function mutateReplica(scope:string,change:(value:TeamReplica)=>void):Promise<TeamReplica>{const db=await database();try{return await new Promise((resolve,reject)=>{const tx=db.transaction("replicas","readwrite"),store=tx.objectStore("replicas");let result:TeamReplica;const fail=(error:unknown)=>{reject(error);tx.abort();};replicaRecord(store,scope,value=>{try{result=value;change(result);validateReplicaCapacity(result);store.put(result);}catch(error){fail(error);}},fail);tx.oncomplete=()=>resolve(result);tx.onerror=tx.onabort=()=>reject(tx.error??new Error("replica_storage"));});}finally{db.close();}}
export async function queueReplicaEdit(scope:string,kind:string,id:string,value:Record<string,unknown>):Promise<TeamReplica>{return mutateReplica(scope,r=>{
 if(!["document","order","project"].includes(kind))throw new Error("replica_kind");
 if(kind==="project"&&r.project_mapping?.local_id===id)id=r.project_mapping.server_id;
 const key=kind+":"+id,old=r.pending.find(p=>p.request.kind===kind&&p.request.entity_id===id);
 if(old&&old.state!=="pending")throw new Error("replica_uncertain");
 const base=old?.base??r.entities[key]??null,expected=Number(base?.revision??0);
 const body={...value,expected_revision:expected,...(kind==="document"?{id}:{})};
 const request={idempotency_key:crypto.randomUUID(),kind,entity_id:id,expected_revision:expected,value:body};
 r.pending=r.pending.filter(p=>p!==old);r.pending.push({request,base,state:"pending"});r.entities[key]={...base,...value,id,revision:expected};
});}
/** Merge independent fields and non-overlapping contiguous text edits. */
export function mergeReplica(base:ReplicaEntity|null,local:Record<string,unknown>,remote:ReplicaEntity|null):{value:Record<string,unknown>,conflicts:string[]}{const value:Record<string,unknown>={},conflicts:string[]=[];for(const key of Object.keys(local).filter(k=>!["id","revision","expected_revision","team_id","project_id","created_at"].includes(k))){const b=base?.[key],l=local[key],r=remote?.[key];if(JSON.stringify(l)===JSON.stringify(b))value[key]=r;else if(JSON.stringify(r)===JSON.stringify(b)||JSON.stringify(l)===JSON.stringify(r))value[key]=l;else if(typeof b==="string"&&typeof l==="string"&&typeof r==="string"){const span=(changed:string)=>{let start=0;while(start<b.length&&start<changed.length&&b[start]===changed[start])start++;let end=b.length,tail=changed.length;while(end>start&&tail>start&&b[end-1]===changed[tail-1]){end--;tail--;}return{start,end,text:changed.slice(start,tail)};};const a=span(l),c=span(r);if(a.end<=c.start&&!(a.start===a.end&&a.start===c.start))value[key]=b.slice(0,a.start)+a.text+b.slice(a.end,c.start)+c.text+b.slice(c.end);else if(c.end<=a.start&&!(c.start===c.end&&c.start===a.start))value[key]=b.slice(0,c.start)+c.text+b.slice(c.end,a.start)+a.text+b.slice(a.end);else{value[key]=l;conflicts.push(key);}}else{value[key]=l;conflicts.push(key);}}return{value,conflicts};}
export async function resolveReplicaConflict(scope:string,key:string,value:Record<string,unknown>):Promise<TeamReplica>{return mutateReplica(scope,r=>{const p=r.pending.find(p=>p.request.idempotency_key===key);if(!p||p.state!=="conflict"||!p.remote)throw new Error("replica_conflict_base");const revision=Number(p.remote.revision);p.base=p.remote;p.request={...p.request,idempotency_key:crypto.randomUUID(),expected_revision:revision,value:{...value,expected_revision:revision,...(p.request.kind==="document"?{id:p.request.entity_id}:{})}};p.state="pending";delete p.remote;r.entities[p.request.kind+":"+p.request.entity_id]={...p.base,...value};});}
export async function discardReplicaMutation(scope:string,key:string):Promise<TeamReplica>{return mutateReplica(scope,r=>{const p=r.pending.find(p=>p.request.idempotency_key===key);if(p?.state==="uncertain")throw new Error("replica_uncertain");if(p){const original=p.remote??p.base,k=p.request.kind+":"+p.request.entity_id;if(original)r.entities[k]=original;else delete r.entities[k];}r.pending=r.pending.filter(p=>p.request.idempotency_key!==key);});}
export async function synchronizeReplica(client:Client,session:string,scope:string,team:string,project:string,deferProjectChanges=false):Promise<TeamReplica>{
 const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
 let binding:unknown;try{binding=JSON.parse(scope);}catch{throw new Error("offline_project_scope");}
 if(!Array.isArray(binding)||binding.length!==4||binding[0]!==client.base||!uuid.test(team)||!uuid.test(project)||binding[2]!==team||binding[3]!==project||typeof binding[1]!=="string"||!session)throw new Error("offline_project_scope");
 const actor=await productRequest<{id:string}>(client,"/identity/me","GET",undefined,session);if(actor.id!==binding[1])throw new Error("offline_project_identity");
 const validate=(kind:string,id:string,value:ReplicaEntity)=>{if(!["document","order","project","run","file"].includes(kind)||!uuid.test(id)||!value||value.id!==id||value.team_id!==team||kind!=="project"&&value.project_id!==project||kind==="project"&&id!==project||["document","order","project"].includes(kind)&&(!Number.isSafeInteger(value.revision)||Number(value.revision)<1))throw new Error("offline_project_receipt");};
 const path=`/teams/${team}/projects/${project}/sync`;let current=await readReplica(scope);
 for(const item of current.pending.filter(p=>p.state!=="conflict"&&(!deferProjectChanges||p.request.kind!=="project")).sort((a,b)=>Number(a.request.kind==="project")-Number(b.request.kind==="project"))){
  let admitted=false;await mutateReplica(scope,r=>{const p=r.pending.find(p=>p.request.idempotency_key===item.request.idempotency_key);if(p&&p.state!=="conflict"){p.state="uncertain";admitted=true;}});if(!admitted)continue;
  try{const receipt=await productRequest<{idempotency_key:string,kind:string,entity_id:string,value:ReplicaEntity}>(client,path,"POST",item.request,session);
   if(receipt.idempotency_key!==item.request.idempotency_key||receipt.kind!==item.request.kind||receipt.entity_id!==item.request.entity_id)throw new Error("offline_project_receipt");validate(receipt.kind,receipt.entity_id,receipt.value);
   await mutateReplica(scope,r=>{if(!r.pending.some(p=>p.request.idempotency_key===item.request.idempotency_key))return;r.entities[item.request.kind+":"+item.request.entity_id]=receipt.value;r.pending=r.pending.filter(p=>p.request.idempotency_key!==item.request.idempotency_key);});
  }catch(e){if(String(e).includes("(409)"))await mutateReplica(scope,r=>{const p=r.pending.find(p=>p.request.idempotency_key===item.request.idempotency_key);if(p)p.state="conflict";});else throw e;}
 }
 for(let page=0;page<100;page++){current=await readReplica(scope);const result=await productRequest<Page>(client,path+`?after=${current.cursor}&limit=100`,"GET",undefined,session);
  if(!Array.isArray(result.changes)||result.changes.length>100||!Number.isSafeInteger(result.next_cursor)||result.next_cursor<current.cursor||result.changes.some(c=>!Number.isSafeInteger(c.cursor)||c.cursor<=current.cursor))throw new Error("replica_cursor");
  for(const c of result.changes)validate(c.kind,c.entity_id,c.payload);
  await mutateReplica(scope,r=>{for(const c of result.changes){const key=c.kind+":"+c.entity_id,p=r.pending.find(p=>p.request.kind===c.kind&&p.request.entity_id===c.entity_id);if(p)p.remote=c.payload;else r.entities[key]=c.payload;}r.cursor=result.next_cursor;r.updated_at=Date.now();});
  if(!result.has_more)return readReplica(scope);if(!result.changes.length)throw new Error("replica_cursor_stalled");
 }
 throw new Error("replica_page_limit");
}
