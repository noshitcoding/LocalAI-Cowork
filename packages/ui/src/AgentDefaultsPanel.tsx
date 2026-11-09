import {useEffect,useRef,useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {decode,defaultRunBudgets,type Client,type PreferencesRecord,type WritePreferences,type ProviderProfile} from "@cowork/client";
import {RunBudgets} from "./RunBudgets";
import {useAgentLibrary} from "./AgentLibrary";
import {useView} from "./view";
import {de,en,type Key} from "./i18n";
import {PersistentRequestLatch,requestJournalEndpoint,definitiveRequestRejection} from "./requestJournal";
class PreferencesError extends Error { constructor(readonly status:number){super("preferences_"+status);} }

export async function preferencesRequest(client:Client,method="GET",body?:unknown):Promise<PreferencesRecord>{
  const response=await fetch(client.base+"/api/v2/preferences",{method,headers:{Authorization:"Bearer "+client.token,...(body?{"Content-Type":"application/json"}:{})},body:body?JSON.stringify(body):undefined,redirect:"error",credentials:"omit",cache:"no-store"});
  if(!response.ok)throw new PreferencesError(response.status);
  const value=decode("PreferencesRecord",await response.json()) as PreferencesRecord;
  if(!Number.isSafeInteger(value.revision)||!value.value||typeof value.value.system_prompt!=="string")throw new Error("preferences_invalid_response");
  if(body&&typeof body==="object"&&"expected_revision" in body&&value.revision!==Number(body.expected_revision)+1)throw new Error("preferences_receipt_mismatch");
  return value;
}
export function useAgentDefaults(client:Client){return useQuery({queryKey:["agent-defaults",client.base,client.token],queryFn:()=>preferencesRequest(client),retry:false});}

export function AgentDefaultsPanel({client,providers}:{client:Client;providers:ProviderProfile[]}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b,t=(key:Key)=>(language==="de"?de:en)[key];
  const query=useAgentDefaults(client),qc=useQueryClient(),library=useAgentLibrary(client);
  const [draft,setDraft]=useState<PreferencesRecord|null>(null),[error,setError]=useState(""),[notice,setNotice]=useState(""),[busy,setBusy]=useState(false),[conflict,setConflict]=useState<PreferencesRecord|null>(null);
  const baseline=useRef<PreferencesRecord|null>(null);
  const pending=useRef(new PersistentRequestLatch<WritePreferences>("cowork.preferences.pending.v1:"+requestJournalEndpoint(client.base)+":installation-owner",raw=>decode("WritePreferences",raw)));
  const [pendingSave,setPendingSave]=useState(pending.current.pending);
  useEffect(()=>{const current=query.data;if(!current||pending.current.invalid)return;if(!draft){if(pendingSave){const original=pending.current.prepare(()=>{throw new Error("preferences_pending");});setDraft({...current,revision:original.expected_revision,value:structuredClone(original.value)});}else{baseline.current=structuredClone(current);setDraft(structuredClone(current));}return;}if(pendingSave)return;if(baseline.current&&current.revision!==baseline.current.revision){if(JSON.stringify(draft.value)===JSON.stringify(baseline.current.value)){baseline.current=structuredClone(current);setDraft(structuredClone(current));setConflict(null);}else setConflict(current);}},[query.data,draft,pendingSave]);
  function change(patch:Partial<PreferencesRecord["value"]>){if(!draft)return;setNotice("");setDraft({...draft,value:{...draft.value,...patch}});}
  async function save(event:FormEvent){event.preventDefault();if(!draft)return;setBusy(true);setError("");try{
    const request=pending.current.prepare(()=>({idempotency_key:crypto.randomUUID(),expected_revision:draft.revision,value:structuredClone(draft.value)}));setPendingSave(true);
    const saved=await preferencesRequest(client,"PUT",request);pending.current.confirm();setPendingSave(false);baseline.current=structuredClone(saved);setDraft(saved);setConflict(null);
    qc.setQueryData(["agent-defaults",client.base,client.token],saved);await qc.invalidateQueries({queryKey:["agent-library"]});setNotice(tr("Standards gespeichert. Sie gelten für neue Chats und Läufe.","Defaults saved. They apply to new chats and runs."));
  }catch(reason){if(reason instanceof PreferencesError&&definitiveRequestRejection(reason.status)&&[400,404,409,422].includes(reason.status)){pending.current.confirm();setPendingSave(false);const latest=await query.refetch();if(latest.data)setConflict(latest.data);}setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}}
  function reload(){if(!query.data)return;baseline.current=structuredClone(query.data);setDraft(structuredClone(query.data));setConflict(null);setError("");}
  function rebase(){if(!conflict||!draft||!baseline.current)return;const changes=Object.fromEntries(Object.entries(draft.value).filter(([key,value])=>JSON.stringify(value)!==JSON.stringify(baseline.current!.value[key as keyof PreferencesRecord["value"]])));baseline.current=structuredClone(conflict);setDraft({...structuredClone(conflict),value:{...conflict.value,...changes}});setConflict(null);setError("");}
  if(query.isPending)return <p role="status">{tr("Agentstandards werden geladen …","Loading agent defaults …")}</p>;
  if(pending.current.invalid)return <div role="alert">{tr("Die lokale Speicherreferenz ist beschädigt. Es wurde keine neue Anfrage gesendet.","The local save reference is damaged. No new request was sent.")} <button onClick={()=>{pending.current.discard();setPendingSave(false);}}>{tr("Lokale Referenz verwerfen","Discard local reference")}</button></div>;
  if(!draft)return <p role="alert">{tr("Agentstandards konnten nicht geladen werden.","Agent defaults could not be loaded.")} <button onClick={()=>void query.refetch()}>{tr("Erneut laden","Retry")}</button></p>;
  return <form className="settings-card" onSubmit={save}><h2>{tr("Standards für Agents & Verhalten","Agent & behavior defaults")}</h2>
    {conflict&&<div role="alert"><p>{tr("Die Einstellungen wurden an anderer Stelle geändert. Dein Entwurf bleibt erhalten.","Settings changed elsewhere. Your draft is retained.")}</p><button type="button" disabled={busy||pendingSave} onClick={reload}>{tr("Aktuellen Stand laden und Entwurf verwerfen","Load current values and discard draft")}</button><button type="button" disabled={busy||pendingSave||!baseline.current} onClick={rebase}>{tr("Eigene Änderungen auf aktuellen Stand übertragen","Rebase my changes onto current values")}</button></div>}
    <fieldset disabled={busy||pendingSave}><label>{tr("Standardmodell","Default model")}<select value={draft.value.default_provider_id??""} onChange={e=>change({default_provider_id:e.target.value||null})}><option value="">{tr("Je Chat auswählen","Choose per chat")}</option>{providers.map(p=><option value={p.id} key={p.id}>{p.name} · {p.model}</option>)}</select></label>
    {[...new Set(providers.map(p=>p.kind))].map(kind=><label key={kind}>{tr("Standard für ","Default for ")+kind}<select value={draft.value.provider_defaults?.[kind]??""} onChange={e=>{const values={...draft.value.provider_defaults};if(e.target.value)values[kind]=e.target.value;else delete values[kind];change({provider_defaults:values});}}><option value="">{tr("Keine Vorgabe","No default")}</option>{providers.filter(p=>p.kind===kind).map(p=><option key={p.id} value={p.id}>{p.name} · {p.model}</option>)}</select></label>)}
    <label>{tr("Standardagent","Default agent")}<select value={draft.value.default_agent_id??""} onChange={e=>change({default_agent_id:e.target.value||null})}><option value="">{tr("Ohne Vorlage","No template")}</option>{library.agents.data?.map(a=><option key={a.id} value={a.id}>{a.value.name}</option>)}</select></label>
    <label>{tr("Zusätzliche Workspace-Anweisungen","Additional workspace instructions")}<textarea rows={6} maxLength={32000} value={draft.value.system_prompt} onChange={e=>change({system_prompt:e.target.value})}/></label>
    <label>{tr("Standardfreigaben","Default approval mode")}<select value={draft.value.default_access_mode} onChange={e=>change({default_access_mode:e.target.value as PreferencesRecord["value"]["default_access_mode"]})}>{(["read_only","confirm_mutations","confirm_all","autonomous"] as const).map(mode=><option key={mode} value={mode}>{t(mode)}</option>)}</select></label>
    <label>{tr("Erlaubte Programmbefehle (eine Zeile pro Muster)","Allowed program commands (one pattern per line)")}<textarea rows={3} value={draft.value.command_whitelist} onChange={e=>change({command_whitelist:e.target.value})}/></label>
    <label>{tr("Gesperrte Programmbefehle","Blocked program commands")}<textarea rows={3} value={draft.value.command_blacklist} onChange={e=>change({command_blacklist:e.target.value})}/></label><p>{tr("Geprüft wird der Programmname mit Argumenten. * steht für beliebige Zeichen; Sperren haben Vorrang. Leere Erlaubnisliste schränkt Programme nicht zusätzlich ein.","Matches the program name and arguments. * spans characters; blocks take precedence. An empty allow list adds no restriction.")}</p>
    <label>{tr("Maximale Werkzeugaufrufe","Maximum tool calls")}<input type="number" min={1} max={4096} value={draft.value.max_tool_calls_per_loop} onChange={e=>change({max_tool_calls_per_loop:Number(e.target.value)})}/></label><label><input type="checkbox" checked={draft.value.fallback_to_human_on_repeated_failure} onChange={e=>change({fallback_to_human_on_repeated_failure:e.target.checked})}/>{tr("Nach drei Werkzeugfehlern um Hinweise bitten","Request guidance after three tool failures")}</label>
    <RunBudgets value={{...defaultRunBudgets,...draft.value.default_budgets}} setValue={default_budgets=>change({default_budgets})} disabled={busy||pendingSave} t={t} contextTokens={undefined}/></fieldset>
    {error&&<p role="alert" className="error">{error}</p>}{notice&&<p role="status">{notice}</p>}
    {pendingSave&&<p>{tr("Die ursprüngliche Speicheranfrage bleibt für eine unveränderte Wiederholung erhalten.","The original save request is retained for an unchanged retry.")}</p>}
    <button type="submit" className="primary" disabled={busy||!!conflict&&!pendingSave}>{tr(pendingSave?"Speicherung erneut klären":"Standards speichern",pendingSave?"Retry original save":"Save defaults")}</button>
  </form>;
}
