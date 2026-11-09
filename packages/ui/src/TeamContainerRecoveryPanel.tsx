import {useEffect, useRef, useState} from "react";
import {Client, type TeamContainerRecoveryPlan, type RetireTeamContainer, type TeamContainerRecoveryReceipt} from "@cowork/client";
import {useView} from "./view";
import {recoveryRequest,recoveryContainerPost} from "./teamRecovery";

type Saved={format:1,scope:string,input:RetireTeamContainer};
const uuid=/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/;
function read(key:string,scope:string):Saved|undefined {
 const raw=localStorage.getItem(key);if(raw===null)return;
 if(raw.length>4096)throw new Error("container_original_intent_invalid");
 const value=JSON.parse(raw) as Saved,input=value?.input;
 if(value?.format!==1||value.scope!==scope||Object.keys(value).sort().join(",")!=="format,input,scope"
  ||!input||Object.keys(input).sort().join(",")!=="allow_legacy_location_adoption,expected_revision,idempotency_key,nonce,plan_sha256"
  ||!uuid.test(input.idempotency_key)||!uuid.test(input.nonce)||!/^[a-f0-9]{64}$/.test(input.plan_sha256)
  ||!Number.isSafeInteger(input.expected_revision)||input.expected_revision<0||typeof input.allow_legacy_location_adoption!=="boolean")
  throw new Error("container_original_intent_invalid");
 return value;
}
function ordered(input:RetireTeamContainer):RetireTeamContainer {
 return {idempotency_key:input.idempotency_key,expected_revision:input.expected_revision,nonce:input.nonce,
  plan_sha256:input.plan_sha256,allow_legacy_location_adoption:input.allow_legacy_location_adoption};
}
async function digest(input:RetireTeamContainer){return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",
 new TextEncoder().encode(JSON.stringify(ordered(input)))))).map(v=>v.toString(16).padStart(2,"0")).join("");}

export function TeamContainerRecoveryPanel({client,session,identity,team,run,onChanged}:{client:Client,session:string,identity:string,team:string,run:string,onChanged:()=>void}) {
 const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
 const scope=JSON.stringify([client.base,identity,team,run]),key="cowork-container-retirement/1:"+scope;
 const path=`/teams/${team}/runs/${run}/container-recovery`,current=useRef(scope);current.current=scope;
 const [plans,setPlans]=useState<TeamContainerRecoveryPlan[]>([]),[selected,setSelected]=useState(""),[reviewed,setReviewed]=useState(false);
 const [pending,setPending]=useState<Saved>(),[receipt,setReceipt]=useState<TeamContainerRecoveryReceipt>(),[error,setError]=useState("");
 const [busy,setBusy]=useState(false),[invalid,setInvalid]=useState(false);const working=useRef(false);
 useEffect(()=>{setBusy(false);setPlans([]);setSelected("");setReviewed(false);setReceipt(undefined);setError("");setInvalid(false);setPending(undefined);
  try{setPending(read(key,scope))}catch(cause){setInvalid(true);setError(String(cause))}},[scope,key]);
 const plan=plans.find(p=>p.nonce===selected);
 async function act(work:()=>Promise<void>,mutation=false){if(working.current||invalid)return;working.current=true;setBusy(true);
  try{if(mutation){if(!navigator.locks)throw new Error("container_original_lock_unavailable");await navigator.locks.request(key+":lock",async()=>{if(current.current===scope)await work()})}
   else await work();if(current.current===scope)setError("");}
  catch(cause){if(current.current===scope)setError(cause instanceof Error?cause.message:String(cause))}
  finally{working.current=false;if(current.current===scope)setBusy(false)}}
 function clear(original:Saved){if(JSON.stringify(read(key,scope))!==JSON.stringify(original))throw new Error("container_original_intent_changed");
  localStorage.removeItem(key);if(current.current===scope)setPending(undefined)}
 async function submit(original:Saved){if(JSON.stringify(read(key,scope))!==JSON.stringify(original))throw new Error("container_original_intent_changed");
  const received=await recoveryContainerPost<TeamContainerRecoveryReceipt>(client,session,path+"/retire",JSON.stringify(ordered(original.input)));
  if(received.operation_id!==original.input.idempotency_key||received.run_id!==run||received.nonce!==original.input.nonce
   ||received.plan_sha256!==original.input.plan_sha256||received.request_sha256!==await digest(original.input)
   ||received.container_removed!==true||received.effects_unknown!==true)throw new Error("container_original_receipt_invalid");
  clear(original);if(current.current===scope){setReceipt(received);setPlans([]);setSelected("");setReviewed(false);onChanged()}}
 async function start(){if(!plan||!reviewed)return;await act(async()=>{
  if(read(key,scope))throw new Error("container_original_intent_pending");
  const saved:Saved={format:1,scope,input:{idempotency_key:crypto.randomUUID(),expected_revision:plan.revision,nonce:plan.nonce,
   plan_sha256:plan.plan_sha256,allow_legacy_location_adoption:plan.legacy_location_adoption}};
  const raw=JSON.stringify(saved);localStorage.setItem(key,raw);if(localStorage.getItem(key)!==raw)throw new Error("container_original_intent_not_retained");
  if(current.current===scope)setPending(saved);await submit(saved)
 },true)}
 return <details className="product-item"><summary>{tr("Alte Container sicher stilllegen","Retire old containers")}</summary>
  <p>{tr("Prüft den ursprünglichen Container am tatsächlich verbundenen Docker-Host. Seine alte Werkzeugwirkung bleibt unbekannt; die Stilllegung führt kein Werkzeug erneut aus.","Checks the original container on the connected Docker host. Its historical tool effect remains unknown; retirement never repeats a tool.")}</p>
  {error&&<p role="alert">{error}</p>}
  <button disabled={busy||invalid||!!pending} onClick={()=>void act(async()=>{const value=await recoveryRequest<TeamContainerRecoveryPlan[]>(client,session,path);if(current.current===scope){setPlans(value);setSelected("");setReviewed(false)}})}>{tr("Originalcontainer prüfen","Inspect original containers")}</button>
  {plans.length>0&&<><label>{tr("Container auswählen","Select container")}<select value={selected} onChange={e=>{setSelected(e.target.value);setReviewed(false)}}><option value=""/>{plans.map(p=><option key={p.nonce} value={p.nonce}>{p.container_id} · {p.host_id}</option>)}</select></label>
   {plan&&<><p>{plan.running?tr("Läuft","Running"):tr("Gestoppt oder am gebundenen Host nicht vorhanden","Stopped or absent on the bound host")} · {plan.engine_id}</p><p>SHA-256: {plan.record_sha256}</p>
    {plan.legacy_location_adoption&&<p>{tr("Diese alte Aufzeichnung besitzt keine ursprüngliche Hostbindung. Der positiv geprüfte aktuelle Standort wird ausschließlich für diesen Stilllegungsauftrag gebunden.","This legacy record has no original host binding. Its positively inspected current location is bound only to this retirement.")}</p>}
    <label><input type="checkbox" checked={reviewed} onChange={e=>setReviewed(e.target.checked)}/>{tr("Genau diesen Container stoppen und entfernen; Originalaufzeichnung und Arbeitsbereich erhalten.","Stop and remove this exact container; retain its original record and workspace.")}</label>
    <button disabled={busy||invalid||!!pending||!reviewed} onClick={()=>void start()}>{tr("Geprüften Container stilllegen","Retire reviewed container")}</button></>}
  </>}
  {pending&&<aside><p>{tr("Der ursprüngliche Auftrag bleibt bis zum belegten Abschluss gespeichert.","The original command remains saved until its completion is established.")} · {pending.input.idempotency_key}</p>
   <button disabled={busy||invalid} onClick={()=>void act(async()=>{const value=await recoveryRequest<TeamContainerRecoveryReceipt|null>(client,session,path+"/operations/"+pending.input.idempotency_key);if(current.current===scope){if(value)setReceipt(value);else throw new Error(tr("Noch kein Originalabschluss bestätigt.","No original completion confirmed yet."))}})}>{tr("Originalquittung nur lesen","Read original receipt only")}</button>
   <button disabled={busy||invalid} onClick={()=>void act(()=>submit(pending),true)}>{tr("Denselben Originalauftrag abschließen","Complete the same original command")}</button>
   <button disabled={busy||invalid} onClick={()=>void act(async()=>{const value=await recoveryContainerPost<{operation_id:string,run_id:string,request_sha256:string,state:string}>(client,session,path+"/reject",JSON.stringify(ordered(pending.input)));
    if(value.state!=="rejected"||value.operation_id!==pending.input.idempotency_key||value.run_id!==run||value.request_sha256!==await digest(pending.input))throw new Error("container_original_rejection_invalid");clear(pending)},true)}>{tr("Nur bei fehlender Annahme dauerhaft ablehnen","Permanently reject only if never admitted")}</button>
  </aside>}
  {receipt&&<details open><summary>{tr("Originaler Stilllegungsbeleg","Original retirement receipt")}</summary><pre>{JSON.stringify(receipt,null,2)}</pre></details>}
 </details>;
}
