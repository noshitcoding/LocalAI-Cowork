import type {AndroidProjectVersion,AndroidProjectMergeReview,AndroidProjectForkReceipt} from "@cowork/client";
export type VersionState={project:{id:string;revision:number;name:string;privacy:string;team_id:string|null;owner_user_id:string|null};current_version_id:string|null;active_snapshot_id:string|null};
export type SnapshotChoice={id:string;project_id:string;created_at:string;total_bytes:number;file_count:number};
export type VersionFile={id:string;path:string;size:number;sha256:string;media_type:string;page_bytes:number};
export type OriginalPost={format:1;actor:string;project:string;endpoint:string;key:string;path:string;body:Record<string,unknown>};
export const uuid=(v:unknown):v is string=>typeof v==="string"&&/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v);
export async function versionRequest<T>(endpoint:string,token:string,path:string,method="GET",body?:unknown,key?:string):Promise<T>{
 const r=await fetch(endpoint+"/api/v1"+path,{method,headers:{Authorization:"Bearer "+token,...(body===undefined?{}:{"Content-Type":"application/json"}),...(key?{"Idempotency-Key":key}:{})},body:body===undefined?undefined:JSON.stringify(body),credentials:"omit",cache:"no-store",redirect:"error"});
 if(!r.ok){let error="project_version_request_failed";try{error=String((await r.json()).error??error);}catch{}throw new Error(`${error} (${r.status})`);}
 const contentRead=method==="GET"&&(path.includes("/copied-content/")||path.includes("/content/")||/\/versions\/[0-9a-f-]{36}\/files$/.test(path)||path.includes("/messages"));
 const limit=contentRead?16*1024*1024+8192:3*1024*1024;
 const reader=r.body?.getReader();if(!reader)throw new Error("project_original_invalid");let size=0;const decoder=new TextDecoder("utf-8",{fatal:true});let raw="";
 try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>limit)throw new Error("project_response_bound");raw+=decoder.decode(part.value,{stream:true});}raw+=decoder.decode();return JSON.parse(raw) as T;}finally{await reader.cancel().catch(()=>{});reader.releaseLock();}
}
export function restoredPost(raw:string,endpoint:string,actor:string,project:string):OriginalPost{
 if(new TextEncoder().encode(raw).length>16*1024*1024)throw new Error("project_original_invalid");const p=JSON.parse(raw) as OriginalPost;
 const prefix=`/projects/${project}`;
 if(p?.format!==1||p.endpoint!==endpoint||p.actor!==actor||p.project!==project||!uuid(p.key)||!p.body||Array.isArray(p.body)||typeof p.body!=="object"||Object.keys(p).some(k=>!["format","actor","project","endpoint","key","path","body"].includes(k))||![prefix+"/versions",prefix+"/merge-apply",prefix+"/fork"].includes(p.path)&&!new RegExp("^"+prefix+"/versions/[0-9a-f-]{36}/apply$","i").test(p.path))throw new Error("project_original_invalid");
 if(p.path.endsWith("/fork")&&p.body.idempotency_key!==p.key)throw new Error("project_original_invalid");return p;
}
export type VersionResult=AndroidProjectVersion|AndroidProjectForkReceipt;
export type VersionReview=AndroidProjectMergeReview;
export async function sha256(bytes:BufferSource){return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(v=>v.toString(16).padStart(2,"0")).join("");}
