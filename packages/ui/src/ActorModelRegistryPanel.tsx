import {useEffect,useRef,useState} from "react";
import {Client,decode,type ActorModelContext,type ActorModelRegistryView} from "@cowork/client";
import {productRequest} from "./productApi";
import {deliverDownload} from "./download";
import {useView} from "./view";

type Item={id:string,name:string,revision:number};
type Intent={format:1,key:string,project:string,profile:string};
const uuid=(value:unknown):value is string=>typeof value==="string"&&/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value)&&value!=="00000000-0000-0000-0000-000000000000";
function pending(key:string):Intent|undefined {
  const text=localStorage.getItem(key);if(!text)return;
  const value:unknown=JSON.parse(text);
  if(!value||typeof value!=="object")throw new Error("model_registry_receipt_invalid");
  const v=value as Record<string,unknown>;
  if(Object.keys(v).sort().join(",")!=="format,key,profile,project"||v.format!==1||!uuid(v.key)||!uuid(v.project)||!uuid(v.profile))throw new Error("model_registry_receipt_invalid");
  return v as Intent;
}

export function ActorModelRegistryPanel({client,session,actorId}:{client:Client;session:string;actorId:string}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [projects,setProjects]=useState<Item[]>([]),[profiles,setProfiles]=useState<Item[]>([]),[project,setProject]=useState(""),[profile,setProfile]=useState(""),[entries,setEntries]=useState<ActorModelRegistryView[]>([]),[context,setContext]=useState<ActorModelContext>(),[intent,setIntent]=useState<Intent>(),[error,setError]=useState(""),[notice,setNotice]=useState(""),[busy,setBusy]=useState(false),[newImport,setNewImport]=useState(false);
  const generation=useRef(0);
  const storageKey=`cowork.actor_model_import.v1:${client.base}:${actorId}`;
  const native=typeof window!=="undefined"&&"__TAURI_INTERNALS__" in window&&client.base==="http://127.0.0.1:43821";
  const api=<T,>(path:string)=>productRequest<T>(client,"/actor-model-registry"+path,"GET",undefined,session);
  const message=(value:unknown)=>{
    const text=value instanceof Error?value.message:typeof value==="object"&&value&&"code" in value?String(value.code):String(value);
    const code=text.split(" (")[0];
    const messages:Record<string,[string,string]>={
      model_registry_receipt_invalid:["Der gespeicherte Importbeleg ist ungültig. Er wurde nicht überschrieben.","The recorded import receipt is invalid. It was not overwritten."],
      model_registry_operation_uncertain:["Die Importbestätigung ist ungewiss. Setze denselben Originalvorgang fort.","The import acknowledgement is uncertain. Continue the same original operation."],
      actor_model_registry_profile_mismatch:["Das Paket gehört nicht zum gewählten Profil. Exportiere die aktuellen Profilmetadaten und qualifiziere genau diese Version.","The bundle does not belong to the selected profile. Export current profile metadata and qualify that exact version."],
      model_registry_retry_cancelled:["Fortsetzung abgebrochen; der Originalauftrag bleibt gespeichert.","Continuation canceled; the original request remains recorded."],
      model_registry_connection_failed:["Die gekoppelte lokale Betreiberverbindung ist nicht verfügbar.","The paired local operator connection is unavailable."],
      actor_model_registry_invalid:["Die private Modellbindung ist ungültig.","The private model binding is invalid."],
      forbidden:["Diese Sitzung darf das private Projekt oder Profil nicht verwenden.","This session may not use the private project or profile."],
      conflict:["Projekt oder Profil wurde geändert oder diese Modellversion ist bereits vergeben. Prüfe den aktuellen Stand.","The project or profile changed, or that model version already exists. Review current state."],
    };
    return messages[code]?.[language==="de"?0:1]??tr("Vorgang fehlgeschlagen: ","Operation failed: ")+code;
  };
  async function attempt(action:(current:number)=>Promise<void>){const current=generation.current;setBusy(true);setError("");setNotice("");try{await action(current);}catch(e){if(current===generation.current)setError(message(e));}finally{if(current===generation.current)setBusy(false);}}
  useEffect(()=>{
    const current=++generation.current;setProjects([]);setProfiles([]);setProject("");setProfile("");setEntries([]);setContext(undefined);setIntent(undefined);setError("");setNotice("");setBusy(false);setNewImport(false);
    if(!session||!uuid(actorId))return;
    try{const saved=pending(storageKey);setIntent(saved);if(saved){setProject(saved.project);setProfile(saved.profile);}}catch(e){setError(message(e));}
    void Promise.all([api<Item[]>("/projects"),api<Item[]>("/profiles")]).then(([p,m])=>{if(current===generation.current){setProjects(p);setProfiles(m);}}).catch(e=>{if(current===generation.current)setError(message(e));});
    return()=>{generation.current++;};
  },[client.base,session,actorId]);
  useEffect(()=>{
    const current=generation.current;let cancelled=false;setEntries([]);setContext(undefined);
    if(!project||!session)return;
    void api<unknown[]>(`/entries?project_id=${encodeURIComponent(project)}`).then(values=>{const rows=values.map(value=>decode("ActorModelRegistryView",value));if(rows.some(row=>row.context.actor_id!==actorId||row.context.project_id!==project))throw new Error("forbidden");if(!cancelled&&current===generation.current)setEntries(rows);}).catch(e=>{if(!cancelled&&current===generation.current)setError(message(e));});
    if(profile)void api<unknown>(`/context?project_id=${encodeURIComponent(project)}&profile_id=${encodeURIComponent(profile)}`).then(value=>{const c=decode("ActorModelContext",value);if(c.actor_id!==actorId||c.project_id!==project||c.provider.id!==profile)throw new Error("forbidden");if(!cancelled&&current===generation.current)setContext(c);}).catch(e=>{if(!cancelled&&current===generation.current)setError(message(e));});
    return()=>{cancelled=true;};
  },[client.base,session,actorId,project,profile]);
  async function reload(current:number){if(!project)return;const rows=(await api<unknown[]>(`/entries?project_id=${encodeURIComponent(project)}`)).map(value=>decode("ActorModelRegistryView",value));if(rows.some(row=>row.context.actor_id!==actorId||row.context.project_id!==project))throw new Error("forbidden");if(current===generation.current)setEntries(rows);}
  async function importBundle(){if(!native||busy||!project||!profile)return;await attempt(async current=>{
    const saved=pending(storageKey);
    if(saved&&(saved.project!==project||saved.profile!==profile))throw new Error("model_registry_receipt_invalid");
    const original=saved??{format:1 as const,key:crypto.randomUUID(),project,profile};
    localStorage.setItem(storageKey,JSON.stringify(original));setIntent(original);
    const {invoke}=await import("@tauri-apps/api/core");
    const result=await invoke<unknown>("native_actor_model_registry_import",{input:{idempotency_key:original.key,project_id:original.project,profile_id:original.profile,actor_session:session},language});
    if(result===null){localStorage.removeItem(storageKey);if(current===generation.current)setIntent(undefined);return;}
    const receipt=decode("ActorModelRegistryView",result);
    if(receipt.context.actor_id!==actorId||receipt.context.project_id!==original.project||receipt.context.provider.id!==original.profile)throw new Error("model_registry_receipt_invalid");
    // Keep the original key for lost-ACK/restart retrieval. Never persist the session.
    if(current===generation.current){setNotice(tr("Originaler privater Importbeleg bestätigt. Die aktuelle Zulassung wird unten angezeigt; neue Läufe müssen die Referenz ausdrücklich wählen.","Original private import receipt confirmed. Current admission is shown below; new runs must explicitly select the reference."));await reload(current);}
  });}
  async function revoke(row:ActorModelRegistryView){if(!native||busy)return;await attempt(async current=>{const {invoke}=await import("@tauri-apps/api/core");await invoke("native_actor_model_registry_revoke",{input:{request:{actor_id:actorId,project_id:project,binding:row.qualification.binding},actor_session:session},language});await reload(current);});}
  async function download(value:unknown,name:string,current:number){const result=await deliverDownload(new Blob([JSON.stringify(value,null,2)+"\n"],{type:"application/json"}),name,language);if(current===generation.current)setNotice(result==="canceled"?tr("Speichern abgebrochen.","Save canceled."):tr("Metadaten gespeichert; Zugangsschlüssel sind nicht enthalten.","Metadata saved; credentials are not included."));}
  return <details className="product-item"><summary>{tr("Private Modellqualifikationen für Android","Private model qualifications for Android")}</summary><p>{tr("Wähle dein tatsächlich privates Projekt und dein eigenes Serverprofil. Die Qualifikation gilt für genau diese Projekt- und Profilrevision. Betreiberpakete benötigen reale Gateway-, Tokenizer-, Template- und unabhängige Referenzbelege. Ohne gewählte Zulassung bleibt der Chat konservativ begrenzt.","Choose your actual private project and your own server profile. Qualification applies to that exact project and profile revision. Operator bundles require actual gateway, tokenizer, template and independent reference evidence. Chat remains conservatively bounded without a selected admission.")}</p>{error&&<p className="error" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
    <label>{tr("Privates Projekt","Private project")}<select value={project} disabled={busy} onChange={e=>setProject(e.target.value)}><option value="">{tr("Auswählen","Select")}</option>{projects.map(p=><option key={p.id} value={p.id}>{p.name} · {p.id}</option>)}{intent&&!projects.some(p=>p.id===intent.project)&&<option value={intent.project}>{tr("Gespeicherter Originalvorgang","Recorded original operation")} · {intent.project}</option>}</select></label>
    <label>{tr("Eigenes Providerprofil","Own provider profile")}<select value={profile} disabled={busy} onChange={e=>setProfile(e.target.value)}><option value="">{tr("Auswählen","Select")}</option>{profiles.map(p=><option key={p.id} value={p.id}>{p.name} · v{p.revision} · {p.id}</option>)}{intent&&!profiles.some(p=>p.id===intent.profile)&&<option value={intent.profile}>{tr("Gespeichertes Originalprofil","Recorded original profile")} · {intent.profile}</option>}</select></label>
    {context&&<><p>{context.provider.endpoint} · {context.provider.model} · {context.provider.context_tokens} {tr("Kontexttokens","context tokens")} · {tr("Profilrevision","Profile revision")} {context.provider.revision}</p><button disabled={busy} onClick={()=>void attempt(current=>download(context.provider,`actor-model-provider-${context.provider.id}.json`,current))}>{tr("Exakte Provider-Metadaten für Gateway exportieren","Export exact provider metadata for the gateway")}</button></>}
    {native?<button disabled={busy||!project||!profile||!!intent&&(intent.project!==project||intent.profile!==profile)} onClick={()=>void importBundle()}>{intent?tr("Gespeicherten Import mit Originalschlüssel fortsetzen","Continue recorded import with original key"):tr("Geprüftes Betreiberpaket nativ importieren","Import reviewed operator bundle natively")}</button>:<p>{tr("Import und Widerruf erfordern die native Betreiberoberfläche des gekoppelten lokalen Servers. Die Browseransicht zeigt deine vorhandenen Zulassungen.","Import and revocation require the native operator interface of the paired local server. This browser view shows your existing admissions.")}</p>}
    <button disabled={busy||!project} onClick={()=>void attempt(reload)}>{tr("Aktuelle Zulassungen laden","Refresh current admissions")}</button>
    {native&&intent&&<><p>{tr("Originalvorgang","Original operation")}: {intent.key}</p><button disabled={busy} onClick={()=>{setProject(intent.project);setProfile(intent.profile);}}>{tr("Gespeicherte Auswahl wiederherstellen","Restore recorded selection")}</button><button disabled={busy} onClick={()=>setNewImport(true)}>{tr("Neuen Import vorbereiten","Prepare a new import")}</button></>}
    {newImport&&<section className="product-item"><p>{tr("Einen neuen Import mit neuem Schlüssel beginnen? Der bisherige native Originalbeleg bleibt erhalten. Eine frühere oder ungewisse Veröffentlichung wird dadurch nicht widerrufen. Ein neues Paket braucht eine neue Modellversion.","Start a new import with a new key? The previous native original receipt remains retained. An earlier or uncertain publication is not revoked. A new bundle needs a new model version.")}</p><button onClick={()=>setNewImport(false)}>{tr("Abbrechen","Cancel")}</button><button disabled={busy} onClick={()=>{try{localStorage.removeItem(storageKey);setIntent(undefined);setNewImport(false);}catch(e){setError(message(e));}}}>{tr("Neuen Vorgang ausdrücklich beginnen","Explicitly start a new operation")}</button></section>}
    {entries.map(row=><article className="product-item" key={row.qualification.binding.id+":"+row.qualification.binding.version}><h4>{row.qualification.binding.id} · v{row.qualification.binding.version} · {row.qualification.revoked?tr("Widerrufen oder Bindung geändert","Revoked or binding changed"):tr("Betreiberqualifiziert","Operator qualified")}</h4><p>{row.context.provider.name} · {row.context.provider.id} · v{row.context.provider.revision} · {row.qualification.context_tokens} {tr("Kontexttokens","context tokens")}</p><dl><dt>{tr("Modell / Deployment","Model / deployment")}</dt><dd>{row.qualification.model_sha256}<br/>{row.qualification.deployment_sha256}</dd><dt>Tokenizer / Template</dt><dd>{row.qualification.tokenizer_sha256}<br/>{row.qualification.chat_template_sha256}</dd><dt>{tr("Originaldatei / Auftrag","Original source / request")}</dt><dd>{row.source_sha256}<br/>{row.request_sha256}</dd></dl><button disabled={busy||row.qualification.revoked} onClick={()=>void attempt(current=>download({actor_id:actorId,project_id:project,project_revision:row.context.project_revision,provider_binding:row.context.provider_binding,model_binding:row.qualification.binding},`actor-model-reference-${row.qualification.binding.version}.json`,current))}>{tr("Referenz für privaten Android-Lauf exportieren","Export reference for a private Android run")}</button>{native&&!row.qualification.revoked&&<button disabled={busy} onClick={()=>void revoke(row)}>{tr("Diese Version nativ widerrufen","Revoke this version natively")}</button>}</article>)}
  </details>;
}
