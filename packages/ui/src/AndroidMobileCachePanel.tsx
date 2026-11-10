import {useEffect,useRef,useState} from "react";
import {androidRequest} from "./androidDeviceApi";
import {useView} from "./view";
type Status={id:string,revision:number,state:string,archive_size:number,archive_sha256:string,entry_count:number,payload_sha256:string|null,source:unknown,archive_parts?:{ordinal:number,size:number,sha256:string}[]};
type Entry={ordinal:number,family:string,source_group:string,source_id:string|null,size:number,sha256:string,target_message_id:string|null,activatable_as_inert_message:boolean};
type Page={items:Entry[],next_cursor:number|null};
type Intent={format:1,scope:string,kind:"begin"|"index"|"activate"|"retain",operation:string,purpose:string,sha256:string,path:string,body:Record<string,unknown>};
const PART=1024*1024,MAX=24*PART,uuid=/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/;
function canonical(v:unknown):string{if(Array.isArray(v))return `[${v.map(canonical).join(",")}]`;if(v&&typeof v==="object")return `{${Object.entries(v).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([k,x])=>`${JSON.stringify(k)}:${canonical(x)}`).join(",")}}`;return JSON.stringify(v);}
async function sha(bytes:Uint8Array){return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",Uint8Array.from(bytes).buffer))).map(v=>v.toString(16).padStart(2,"0")).join("");}
async function bodySha(value:unknown){return sha(new TextEncoder().encode(canonical(value)));}
function pending(key:string,scope:string):Intent|undefined{
 const raw=localStorage.getItem(key);if(raw===null)return;if(raw.length>8192)throw new Error("cache_original_intent_invalid");
 const v=JSON.parse(raw) as Intent;
 if(v?.format!==1||v.scope!==scope||!["begin","index","activate","retain"].includes(v.kind)||!/^[a-f0-9]{64}$/.test(v.sha256)
  ||!v.body||typeof v.body!=="object"||Array.isArray(v.body)||typeof v.path!=="string"
  ||!/^\/mobile-cache\/imports(?:\/[a-f0-9-]{36}\/(?:index|activate|retain))?$/.test(v.path)
  ||typeof v.operation!=="string"||!/^cache-(?:begin|activate):[a-f0-9-]{36}$|^cache-(?:index|retain):[a-f0-9-]{36}:\d+$/.test(v.operation)
  ||!["cache_begin","cache_index","cache_activate","cache_retain"].includes(v.purpose))throw new Error("cache_original_intent_invalid");
 return v;
}

export function AndroidMobileCachePanel({endpoint,token,actorId,threadId,onChanged}:{endpoint:string,token:string,actorId:string,threadId?:string,onChanged?:()=>void}){
 const lang=useView(v=>v.language),tr=(de:string,en:string)=>lang==="de"?de:en;
 const scope=JSON.stringify([endpoint,actorId]),key="cowork-mobile-cache-import/1:"+scope,current=useRef(scope),epoch=useRef(0);current.current=scope;
 const [list,setList]=useState<Status[]>([]),[id,setId]=useState(""),[status,setStatus]=useState<Status>();
 const [page,setPage]=useState<Page>({items:[],next_cursor:null}),[cursors,setCursors]=useState([0]),[selection,setSelection]=useState<number[]>([]);
 const [file,setFile]=useState<File>(),[fileInfo,setFileInfo]=useState<{size:number,sha256:string}>(),[reviewed,setReviewed]=useState(false),[reason,setReason]=useState("");
 const [intent,setIntent]=useState<Intent>(),[invalid,setInvalid]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(""),[receipt,setReceipt]=useState<unknown>(),[preview,setPreview]=useState<string>();
 const working=useRef(false),after=cursors[cursors.length-1];
 const api=<T,>(path:string,method="GET",body?:unknown)=>androidRequest<T>(endpoint,token,path,method,body);
 async function validateIntent(original:Intent){
  const body=original.body;let fingerprint:unknown,operation:string;
  if(original.purpose!=="cache_"+original.kind)throw new Error("cache_original_intent_invalid");
  if(original.kind==="begin"){
   if(original.path!=="/mobile-cache/imports"||typeof body.request_id!=="string"||!uuid.test(body.request_id)
    ||!Number.isSafeInteger(body.archive_size)||Number(body.archive_size)<1||Number(body.archive_size)>MAX
    ||typeof body.archive_sha256!=="string"||!/^[a-f0-9]{64}$/.test(body.archive_sha256)
    ||Object.keys(body).sort().join(",")!=="archive_sha256,archive_size,request_id")throw new Error("cache_original_intent_invalid");
   fingerprint=body;operation="cache-begin:"+body.request_id;
  }else{
   const match=/^\/mobile-cache\/imports\/([a-f0-9-]{36})\/(index|activate|retain)$/.exec(original.path);
   if(!match||!uuid.test(match[1])||match[2]!==original.kind||!Number.isSafeInteger(body.expected_revision)||Number(body.expected_revision)<1)throw new Error("cache_original_intent_invalid");
   if(original.kind==="activate"){
    if(typeof body.request_id!=="string"||!uuid.test(body.request_id)||typeof body.thread_id!=="string"||!uuid.test(body.thread_id)
     ||!Array.isArray(body.ordinals)||body.ordinals.length<1||body.ordinals.length>100||new Set(body.ordinals).size!==body.ordinals.length
     ||!body.ordinals.every(value=>Number.isSafeInteger(value)&&Number(value)>0)||typeof body.remainder_history_only_reason!=="string"
     ||!body.remainder_history_only_reason.trim()||body.remainder_history_only_reason.length>2048
     ||Object.keys(body).sort().join(",")!=="expected_revision,ordinals,remainder_history_only_reason,request_id,thread_id")throw new Error("cache_original_intent_invalid");
    fingerprint={import_id:match[1],request_id:body.request_id,expected_revision:body.expected_revision,thread_id:body.thread_id,entries:body.ordinals,history_only_reason:body.remainder_history_only_reason};operation="cache-activate:"+body.request_id;
   }else{
    if(Object.keys(body).join(",")!=="expected_revision")throw new Error("cache_original_intent_invalid");
    const source=await api<Status>("/mobile-cache/imports/"+match[1]);
    if(source.id!==match[1]||!/^[a-f0-9]{64}$/.test(source.archive_sha256))throw new Error("cache_original_import_invalid");
    fingerprint={import_id:match[1],expected_revision:body.expected_revision,archive_sha256:source.archive_sha256};operation=`cache-${original.kind}:${match[1]}:${body.expected_revision}`;
   }
  }
  if(operation!==original.operation||await bodySha(fingerprint)!==original.sha256)throw new Error("cache_original_intent_digest_invalid");
 }
 useEffect(()=>{setReviewed(false)},[threadId]);
 useEffect(()=>{epoch.current++;setBusy(false);setId("");setList([]);setStatus(undefined);setPage({items:[],next_cursor:null});setFile(undefined);setFileInfo(undefined);setReviewed(false);setSelection([]);setCursors([0]);setReceipt(undefined);setPreview(undefined);setError("");setInvalid(false);setIntent(undefined);
  try{setIntent(pending(key,scope))}catch(cause){setInvalid(true);setError(String(cause))}},[scope,key]);
 async function act(work:(generation:number)=>Promise<void>,mutation=false){if(working.current||invalid)return;const generation=epoch.current;working.current=true;setBusy(true);
  try{if(mutation){if(!navigator.locks)throw new Error("cache_original_lock_unavailable");await navigator.locks.request(key+":lock",async()=>{if(current.current===scope&&epoch.current===generation)await work(generation)})}else await work(generation);
   if(epoch.current===generation)setError("")}
  catch(cause){if(epoch.current===generation)setError(cause instanceof Error?cause.message:String(cause))}
  finally{working.current=false;if(epoch.current===generation)setBusy(false)}}
 async function reload(generation:number,selected=id,cursor=after){const all=await api<Status[]>("/mobile-cache/imports");let value:Status|undefined,entries:Page={items:[],next_cursor:null};
  if(selected){value=await api<Status>("/mobile-cache/imports/"+selected);if(value.state==="indexed")entries=await api<Page>(`/mobile-cache/imports/${selected}/entries?after=${cursor}&limit=50`)}
  if(epoch.current===generation){setList(all);setStatus(value);setPage(entries)}}
 function clear(original:Intent){if(canonical(pending(key,scope))!==canonical(original))throw new Error("cache_original_intent_changed");localStorage.removeItem(key);if(current.current===scope)setIntent(undefined)}
 async function operation(kind:Intent["kind"],path:string,body:Record<string,unknown>,originalBody:unknown,operation:string,purpose:string,generation:number){
  const value:Intent={format:1,scope,kind,path,body,operation,purpose,sha256:await bodySha(originalBody)};
  const old=pending(key,scope);if(old&&canonical(old)!==canonical(value))throw new Error("cache_original_intent_pending");
  const raw=JSON.stringify(value);localStorage.setItem(key,raw);if(localStorage.getItem(key)!==raw)throw new Error("cache_original_intent_not_retained");if(epoch.current===generation)setIntent(value);
  const answer=await api<unknown>(path,"POST",body);clear(value);if(epoch.current===generation)setReceipt(answer);return answer;
 }
 async function binary(path:string,body?:Uint8Array):Promise<{bytes:Uint8Array,response:Response}>{
  const controller=new AbortController(),timer=setTimeout(()=>controller.abort(),30000);
  try{const response=await fetch(endpoint+"/api/v1"+path,{method:body?"PUT":"GET",headers:{Authorization:"Bearer "+token,...(body?{"Content-Type":"application/octet-stream"}:{})},
   body:body?new Blob([Uint8Array.from(body).buffer]):undefined,credentials:"omit",cache:"no-store",redirect:"error",signal:controller.signal});
   if(!response.ok)throw new Error(`cache_bytes_request_failed (${response.status})`);if(!response.body)throw new Error("cache_bytes_missing");
   const reader=response.body.getReader(),parts:Uint8Array[]=[];let size=0;try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.length;if(size>PART){await reader.cancel();throw new Error("cache_bytes_too_large")}parts.push(part.value)}}finally{reader.releaseLock()}
   const bytes=new Uint8Array(size);let offset=0;for(const part of parts){bytes.set(part,offset);offset+=part.length}return {bytes,response};
  }finally{clearTimeout(timer)}
 }
 async function originalPart(importId:string,stream:string,ordinal:number):Promise<Uint8Array>{const {bytes,response}=await binary(`/mobile-cache/imports/${importId}/bytes?stream=${encodeURIComponent(stream)}&ordinal=${ordinal}`);
  const size=response.headers.get("x-content-size"),digest=response.headers.get("x-content-sha256");if(!size||!/^\d+$/.test(size)||Number(size)!==bytes.length||!digest||!/^[a-f0-9]{64}$/.test(digest)||await sha(bytes)!==digest)throw new Error("cache_original_bytes_invalid");return bytes}
 async function inspectFile(selected:File){if(selected.size<1||selected.size>MAX)throw new Error(tr("Export muss höchstens 24 MiB groß sein.","Export must be at most 24 MiB."));const digest=await sha(new Uint8Array(await selected.arrayBuffer()));return {size:selected.size,sha256:digest}}
 async function begin(){if(!file||!fileInfo||!reviewed)return;await act(async generation=>{
  if(pending(key,scope))throw new Error("cache_original_intent_pending");const actual=await inspectFile(file);if(canonical(actual)!==canonical(fileInfo))throw new Error("cache_selected_file_changed");
  const request=crypto.randomUUID(),body={request_id:request,archive_size:actual.size,archive_sha256:actual.sha256};const answer=await operation("begin","/mobile-cache/imports",body,body,"cache-begin:"+request,"cache_begin",generation) as Status;
  if(!uuid.test(answer.id))throw new Error("cache_original_import_invalid");if(epoch.current===generation){setId(answer.id);setCursors([0])}await reload(generation,answer.id,0)
 },true)}
 async function upload(){if(!file||!id)return;await act(async generation=>{
  const actual=await inspectFile(file),original=await api<Status>("/mobile-cache/imports/"+id);
  if(original.state!=="receiving"||original.archive_sha256!==actual.sha256||original.archive_size!==actual.size)throw new Error("cache_original_file_differs");
  for(const part of original.archive_parts??[]){if(!Number.isSafeInteger(part.ordinal)||part.ordinal<0||part.ordinal*PART>=file.size)throw new Error("cache_original_part_invalid");const raw=new Uint8Array(await file.slice(part.ordinal*PART,(part.ordinal+1)*PART).arrayBuffer());if(raw.length!==part.size||await sha(raw)!==part.sha256)throw new Error("cache_original_part_differs")}
  for(let ordinal=0;ordinal*PART<file.size;ordinal++){if(epoch.current!==generation)throw new Error("cache_context_changed");const bytes=new Uint8Array(await file.slice(ordinal*PART,(ordinal+1)*PART).arrayBuffer());if(original.archive_parts?.some(p=>p.ordinal===ordinal))continue;
   await binary(`/mobile-cache/imports/${id}/parts/${ordinal}`,bytes)}await reload(generation,id,0)
 },true)}
 async function index(){if(!status)return;await act(async generation=>{const original={import_id:status.id,expected_revision:status.revision,archive_sha256:status.archive_sha256};await operation("index",`/mobile-cache/imports/${status.id}/index`,{expected_revision:status.revision},original,`cache-index:${status.id}:${status.revision}`,"cache_index",generation);await reload(generation,status.id,0)},true)}
 async function retain(){if(!status)return;await act(async generation=>{const original={import_id:status.id,expected_revision:status.revision,archive_sha256:status.archive_sha256};await operation("retain",`/mobile-cache/imports/${status.id}/retain`,{expected_revision:status.revision},original,`cache-retain:${status.id}:${status.revision}`,"cache_retain",generation);await reload(generation,status.id,0)},true)}
 async function activate(){if(!status||!threadId||!selection.length||!reason.trim()||!reviewed)return;await act(async generation=>{const request=crypto.randomUUID(),entries=[...selection].sort((a,b)=>a-b),body={request_id:request,expected_revision:status.revision,thread_id:threadId,ordinals:entries,remainder_history_only_reason:reason.trim()};
  await operation("activate",`/mobile-cache/imports/${status.id}/activate`,body,{import_id:status.id,request_id:request,expected_revision:status.revision,thread_id:threadId,entries,history_only_reason:reason.trim()},"cache-activate:"+request,"cache_activate",generation);
  if(epoch.current===generation){setSelection([]);setReviewed(false);onChanged?.()}await reload(generation,status.id,after)
 },true)}
 async function recover(original:Intent,submit=false){await act(async generation=>{await validateIntent(original);if(epoch.current!==generation)throw new Error("cache_context_changed");if(submit){const value=await api<unknown>(original.path,"POST",original.body);clear(original);if(epoch.current===generation)setReceipt(value)}else{const value=await api<unknown>(`/administration/operations/${encodeURIComponent(original.operation)}?purpose=${original.purpose}&request_sha256=${original.sha256}`);if(value===null)throw new Error("cache_original_receipt_unconfirmed");clear(original);if(epoch.current===generation)setReceipt(value)}await reload(generation)},submit)}
 async function openEntry(entry:Entry){if(!status)return;await act(async generation=>{const parts:Uint8Array[]=[];let size=0;for(let ordinal=0;size<entry.size;ordinal++){if(ordinal>8)throw new Error("cache_entry_too_large");const bytes=await originalPart(status.id,"entry:"+entry.ordinal,ordinal);if(!bytes.length||bytes.length>entry.size-size)throw new Error("cache_entry_bytes_invalid");parts.push(bytes);size+=bytes.length}const bytes=new Uint8Array(size);let offset=0;for(const part of parts){bytes.set(part,offset);offset+=part.length}if(await sha(bytes)!==entry.sha256)throw new Error("cache_entry_digest_invalid");if(epoch.current===generation)setPreview(new TextDecoder("utf-8",{fatal:true}).decode(bytes))})}
 async function downloadArchive(){if(!status)return;await act(async()=>{const parts:Uint8Array[]=[];let size=0;for(let ordinal=0;size<status.archive_size;ordinal++){if(ordinal>=24)throw new Error("cache_archive_too_large");const bytes=await originalPart(status.id,"archive",ordinal);if(!bytes.length||bytes.length>status.archive_size-size)throw new Error("cache_archive_bytes_invalid");parts.push(bytes);size+=bytes.length}const bytes=new Uint8Array(size);let offset=0;for(const part of parts){bytes.set(part,offset);offset+=part.length}if(await sha(bytes)!==status.archive_sha256)throw new Error("cache_archive_digest_invalid");const url=URL.createObjectURL(new Blob([bytes.buffer],{type:"application/json"})),anchor=document.createElement("a");anchor.href=url;anchor.download=`mobile-cache-${status.archive_sha256}.json`;anchor.click();setTimeout(()=>URL.revokeObjectURL(url),1000)})}
 return <details className="product-item"><summary>{tr("Alten Mobile-Cache übernehmen","Import original mobile cache")}</summary>
  <p>{tr("Wähle den Originalexport aus der bestehenden Mobile-App. Alte Nachrichten können ausdrücklich in den ausgewählten Thread kopiert werden. Outbox-Aktionen und Laufzustände bleiben Historie.","Choose the original export from the existing mobile app. Historical messages can be explicitly copied to the selected thread. Outbox actions and run states remain history.")}</p>
  {error&&<p role="alert">{error}</p>}<button disabled={busy||invalid} onClick={()=>void act(generation=>reload(generation))}>{tr("Gespeicherte Importe laden","Load stored imports")}</button>
  <label>{tr("Originalexport auswählen","Choose original export")}<input type="file" accept="application/json,.json" disabled={busy||invalid} onChange={e=>{const selected=e.target.files?.[0];if(selected)void act(async generation=>{const info=await inspectFile(selected);if(epoch.current===generation){setFile(selected);setFileInfo(info);setReviewed(false)}})}}/></label>
  {fileInfo&&<p>{fileInfo.size.toLocaleString()} bytes · SHA-256: {fileInfo.sha256}</p>}
  <label><input type="checkbox" checked={reviewed} onChange={e=>setReviewed(e.target.checked)}/>{tr("Ich habe die Originaldatei und die ausgewählten Zielinhalte geprüft.","I reviewed the original file and selected target content.")}</label>
  <button disabled={busy||invalid||!!intent||!fileInfo||!reviewed} onClick={()=>void begin()}>{tr("Neuen Quarantäneimport anlegen","Create new quarantine import")}</button>
  <label>{tr("Gespeicherter Import","Stored import")}<select value={id} disabled={busy} onChange={e=>{const selected=e.target.value;setId(selected);setCursors([0]);setSelection([]);setReviewed(false);setPreview(undefined);void act(generation=>reload(generation,selected,0))}}><option value=""/>{list.map(v=><option key={v.id} value={v.id}>{v.state} · {v.id}</option>)}</select></label>
  {status&&<><p>{status.state} · {status.entry_count} {tr("historische Einträge","historical entries")} · {status.archive_sha256}</p>
   {status.state==="receiving"&&<><button disabled={busy||!!intent||!file} onClick={()=>void upload()}>{tr("Dieselben Originalteile hochladen / fortsetzen","Upload or continue the same original parts")}</button><button disabled={busy||!!intent} onClick={()=>void index()}>{tr("Originaldatei prüfen und indizieren","Verify and index original file")}</button>
    <button disabled={busy||!!intent} onClick={()=>void retain()}>{tr("Bisherige Daten als Historie erhalten","Retain received data as history")}</button></>}
   <button disabled={busy} onClick={()=>void downloadArchive()}>{tr("Hashgeprüften Originalexport herunterladen","Download hash-verified original export")}</button>
   <details><summary>{tr("Ursprüngliche Herkunft","Original provenance")}</summary><pre>{JSON.stringify(status.source,null,2)}</pre></details>
   {page.items.map(entry=><article className="product-item" key={entry.ordinal}><p>{entry.family} · {entry.source_group} · {entry.source_id??entry.ordinal}</p><p>{entry.size} bytes · {entry.sha256}</p><button disabled={busy} onClick={()=>void openEntry(entry)}>{tr("Unveränderten Eintrag lesen","Read immutable entry")}</button>
    {entry.activatable_as_inert_message&&!entry.target_message_id&&<label><input type="checkbox" disabled={busy||!!intent} checked={selection.includes(entry.ordinal)} onChange={e=>{setSelection(old=>e.target.checked?[...old,entry.ordinal]:old.filter(v=>v!==entry.ordinal));setReviewed(false)}}/>{tr("Als historische Nachricht kopieren","Copy as historical message")}</label>}{entry.target_message_id&&<p>{tr("Bereits kopiert","Already copied")} · {entry.target_message_id}</p>}</article>)}
   <div className="actions">{cursors.length>1&&<button disabled={busy} onClick={()=>void act(async generation=>{const next=cursors.slice(0,-1);setCursors(next);await reload(generation,id,next[next.length-1])})}>{tr("Vorherige Seite","Previous page")}</button>}{page.next_cursor!==null&&<button disabled={busy} onClick={()=>void act(async generation=>{setCursors(old=>[...old,page.next_cursor!]);await reload(generation,id,page.next_cursor!)})}>{tr("Weitere Originaleinträge","More original entries")}</button>}</div>
   {status.state==="indexed"&&<><p>{tr("Ziel ist der aktuell ausgewählte Thread.","Target is the currently selected thread.")} · {threadId??tr("Kein Thread ausgewählt","No thread selected")}</p><label>{tr("Grund für die verbleibende Historie","Reason for remaining history")}<input value={reason} maxLength={2048} onChange={e=>{setReason(e.target.value);setReviewed(false)}}/></label><button disabled={busy||invalid||!!intent||!threadId||!selection.length||selection.length>100||!reason.trim()||!reviewed} onClick={()=>void activate()}>{tr("Ausgewählte Nachrichten übernehmen","Import selected historical messages")}</button></>}
  </>}
  {intent&&<aside><p>{tr("Originalauftrag noch unbestätigt","Original command unconfirmed")} · {intent.operation}</p><button disabled={busy||invalid} onClick={()=>void recover(intent)}>{tr("Originalquittung nur lesen","Read original receipt only")}</button><button disabled={busy||invalid} onClick={()=>void recover(intent,true)}>{tr("Denselben Originalauftrag erneut übergeben","Submit the same original command again")}</button></aside>}
  {preview&&<pre className="product-preview">{preview}</pre>}{receipt!==undefined&&<details><summary>{tr("Unveränderte Originalquittung","Immutable original receipt")}</summary><pre>{JSON.stringify(receipt,null,2)}</pre></details>}
 </details>;
}
