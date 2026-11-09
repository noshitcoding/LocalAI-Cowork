import {HistoricalLibraryAdoption} from "./HistoricalLibraryAdoption";
import {useTeamText} from "./teamStrings";
import {useEffect,useRef,useState,type FormEvent} from "react";
import {Client,decode,defaultRunBudgets,type CreateProvider,type ProviderProfile,type WorkflowDefinition} from "@cowork/client";
import {productRequest} from "./productApi";
import type {ImportedConfiguration,MigrationWorkflow} from "./v1Migration";

function providerKind(endpoint:string):CreateProvider["kind"]{
 try{const u=new URL(endpoint);if(u.port==="11434"&&!u.pathname.includes("/v1"))return "ollama";}catch{}
 return "openai_compatible";
}
function ImportedProviderSetup({configuration:c,client,disabled,complete}:{configuration:ImportedConfiguration,client:Client,disabled:boolean,complete:(profile:ProviderProfile)=>Promise<void>}){
  const tr=useTeamText();
 const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();const [notice,setNotice]=useState("");
 const pending=useRef<{publicFingerprint:string,key:string}|null>(null);
 const native=typeof window!=="undefined"&&"__TAURI_INTERNALS__" in window&&client.base==="http://127.0.0.1:43821"&&sessionStorage.getItem("cowork-v2-connection-kind")==="native";
 async function submit(event:FormEvent<HTMLFormElement>){
  event.preventDefault();const form=event.currentTarget;const values=new FormData(form);const text=(name:string)=>String(values.get(name)??"");
  const profile:CreateProvider={name:text("name"),kind:text("kind") as CreateProvider["kind"],endpoint:text("endpoint"),model:text("model"),context_tokens:Number(text("context"))};
  const anonymous=values.get("anonymous")==="on";const apiKey=text("api_key");
  const field=form.elements.namedItem("api_key");if(field instanceof HTMLInputElement)field.value="";
  const publicFingerprint=JSON.stringify({profile,anonymous});
  if(!pending.current||pending.current.publicFingerprint!==publicFingerprint)pending.current={publicFingerprint,key:crypto.randomUUID()};
  const input={profile,idempotency_key:pending.current.key,api_key:anonymous?null:apiKey,explicitly_allow_no_key:anonymous};
  setBusy(true);setNotice("");try{
   if(anonymous&&apiKey)throw new Error(tr("Schlüssel löschen oder den Betrieb ohne Schlüssel deaktivieren."));
   if(!anonymous&&!apiKey)throw new Error(tr("Einen neuen eigenen API-Schlüssel eingeben oder schlüssellosen Betrieb ausdrücklich freigeben."));
   let configured:ProviderProfile;
   if(native){const {invoke}=await import("@tauri-apps/api/core");configured=decode("ProviderProfile",await invoke("configure_personal_provider",{input}));}
   else{
    const u=new URL(client.base||location.origin);
    if(u.protocol!=="https:"&&!(u.protocol==="http:"&&["127.0.0.1","localhost","[::1]"].includes(u.hostname)))throw new Error(tr("Persönliche Schlüssel nur über HTTPS oder den eigenen Loopback-Server konfigurieren."));
    configured=await client.one("/providers/with-credential","ProviderProfile","POST",input);
   }
   await complete(configured);pending.current=null;setError(undefined);
   setNotice(`Neuer V2-Provider „${configured.name}“ tatsächlich gespeichert (${configured.id}); noch kein Lauf gestartet.`);
  }catch(e){setError(e);}finally{setBusy(false);}
 }
 return <form onSubmit={event=>void submit(event)}><h4>{tr("Persönlichen Provider neu konfigurieren")}</h4>
  <p>{tr("Historisch: ")}{c.source.base_url??tr("kein Endpunkt")} · {c.source.model??tr("kein Modell")}{tr(". Diese Werte und der Zugang werden jetzt ausdrücklich neu freigegeben. Alte Schlüssel und Providerrechte werden nicht übernommen.")}</p>
  {!!error&&<p className="error" role="alert">{error instanceof Error?error.message:error&&typeof error==="object"&&"code" in error?String(error.code):String(error)}</p>}{notice&&<p role="status">{notice}</p>}
  <fieldset disabled={busy||disabled}>
  <label>{tr("Name")}<input name="name" required maxLength={100} defaultValue={c.source.name}/></label>
  <label>{tr("API-Typ")}<select name="kind" defaultValue={providerKind(c.source.base_url??"")}><option value="ollama">Ollama (native API)</option><option value="openai_compatible">{tr("OpenAI-kompatibel / LiteLLM / vLLM / OpenRouter")}</option></select></label>
  <label>{tr("Aktueller Modellendpunkt")}<input name="endpoint" type="url" required defaultValue={c.source.base_url??""} placeholder="https://mein-litellm.example/v1"/></label>
  <label>{tr("Aktuelle Modellbezeichnung")}<input name="model" required maxLength={200} defaultValue={c.source.model??""}/></label>
  <label>{tr("Kontextfenster")}<input name="context" type="number" min={1024} max={2000000} required defaultValue={32768}/></label>
  <label>{tr("Neu eingegebener eigener API-Schlüssel")}<input name="api_key" type="password" maxLength={4096} autoComplete="off"/></label>
  <label className="product-check"><input name="anonymous" type="checkbox"/>{tr("Diesen Endpunkt ausdrücklich ohne API-Schlüssel verwenden (z.B. eigenes lokales Ollama)")}</label>
  <label className="product-check"><input type="checkbox" required/>{tr("Neues V2-Profil mit genau diesen aktuellen Werten und diesem Zugang freigeben.")}</label>
  <p className="muted">{native?tr("Der Schlüssel wird ausschließlich per nativer IPC und privatem Daemon-Header übertragen."):tr("Der neue Schlüssel wird an den aktuell gewählten eigenen Cowork-Server übertragen.")}{tr(" Er wird privat verschlüsselt und an dieses unveränderliche Providerprofil gebunden gespeichert; Importdaten und Browserstorage erhalten ihn nicht.")}</p>
  <p className="muted">{tr("Bei unbestätigtem Speichern denselben Schlüssel neu eingeben und dieselbe Anfrage wiederholen. Für einen bewusst anderen Zugang eine neue Konfiguration beginnen.")}</p>
  <button className="primary">{tr("Neues Profil und Zugang tatsächlich speichern")}</button>
  <button type="button" onClick={()=>{pending.current=null;setNotice(tr("Die nächste ausdrücklich abgesendete Konfiguration erhält eine neue Anfrage-ID."));}}>{tr("Neue Konfiguration beginnen")}</button>
  </fieldset></form>;
}

function cron(value:string|null){const a=(value??"").trim().split(/\s+/);if(a.length===5&&/^\d+$/.test(a[0])&&/^\d+$/.test(a[1])&&a[2]==="*"&&a[3]==="*"){const hour=Number(a[1]),minute=Number(a[0]);if(hour<24&&minute<60){if(a[4]==="*")return {kind:"daily",hour,minute};if(/^[0-7]$/.test(a[4]))return {kind:"weekly",weekday:Number(a[4])||7,hour,minute};}}return null;}
export function ImportedConfigurations({client,project,team,session,refreshSignal=false}:{client:Client,project:string,team?:string,session?:string,refreshSignal?:boolean}){
  const tr=useTeamText();
 const [items,setItems]=useState<ImportedConfiguration[]>([]);const [providers,setProviders]=useState<ProviderProfile[]>([]);const [provider,setProvider]=useState("");const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();const [notice,setNotice]=useState("");const [prompt,setPrompt]=useState<Record<string,string>>({});const [plans,setPlans]=useState<Record<string,string>>({});const [recurrences,setRecurrences]=useState<Record<string,string>>({});const [prepared,setPrepared]=useState<Record<string,{id:string,revision:number}>>({});const intents=useRef(new Map<string,string>());
 const base=team?`/teams/${team}/projects/${project}`:`/projects/${project}`;const api=<T,>(path:string,method="GET",body?:unknown)=>productRequest<T>(client,path,method,body,team?session:client.token);
 async function refresh(){setItems(await api<ImportedConfiguration[]>(base+"/import-v1/configurations"));if(!team){const p=await client.providers();setProviders(p);setProvider(current=>p.some(v=>v.id===current)?current:p.find(v=>v.credential_ready)?.id??p[0]?.id??"");}}
 useEffect(()=>{setItems([]);setProviders([]);setProvider("");setPrepared({});setPlans({});setPrompt({});setRecurrences({});setNotice("");setError(undefined);intents.current.clear();if(project)void refresh().catch(setError);},[client.base,team,session,project,refreshSignal]);
 async function attempt(work:()=>Promise<void>){setBusy(true);try{await work();setError(undefined);}catch(e){setError(e);}finally{setBusy(false);}}
 function key(id:string,action:string){const slot=id+":"+action;let value=intents.current.get(slot);if(!value){value=crypto.randomUUID();intents.current.set(slot,value);}return value;}
 function currentPrompt(c:ImportedConfiguration){return prompt[c.id]??c.source.instructions??"";}
 function workflow(c:ImportedConfiguration):MigrationWorkflow{const result=JSON.parse(plans[c.id]??JSON.stringify(c.source.workflow));if(!result||!Array.isArray(result.agents)||!Array.isArray(result.steps))throw new Error(tr("Workflow zuerst Agenten und Schritten zuordnen"));return result as MigrationWorkflow;}
 async function prepare(c:ImportedConfiguration){await attempt(async()=>{if(prepared[c.id])return;const text=currentPrompt(c);if(!text.trim())throw new Error(tr("Heutigen Auftragstext ausdrücklich eingeben"));let order=team?await api<{id:string,revision:number}>(base+"/orders","POST",{title:c.source.name,prompt:text,idempotency_key:key(c.id,"order")}):await client.createOrder({project_id:project,title:c.source.name});if(!team&&text)order=await client.saveDraft(order.id,{base_revision:order.revision,draft:text});setPrepared(v=>({...v,[c.id]:order}));setNotice(tr("Neuer Auftrag erstellt; noch keine Ausführung gestartet."));});}
 async function start(c:ImportedConfiguration){const order=prepared[c.id];if(!order||!team)return;await attempt(async()=>{if(!currentPrompt(c).trim())throw new Error(tr("Heutigen Auftragstext ausdrücklich eingeben"));await api(`/teams/${team}/runs`,"POST",{order_id:order.id,prompt:currentPrompt(c),idempotency_key:key(c.id,"run"),plan:{workflow:workflow(c),tools:[],skills:[],knowledge:[]}});setNotice(tr("Neuer Workflowlauf mit heutigen Teamrechten gestartet."));});}
 async function schedule(c:ImportedConfiguration){const order=prepared[c.id];if(!order)return;await attempt(async()=>{if(!currentPrompt(c).trim())throw new Error(tr("Heutigen Auftragstext ausdrücklich eingeben"));const recurrence=JSON.parse(recurrences[c.id]??JSON.stringify(cron(c.source.schedule_expression)));if(!recurrence)throw new Error(tr("Zeitmuster explizit einem V2-Zeitmuster zuordnen"));if(team){const current=await api<{revision:number}>(base);const provider=await api<{revision:number,key_configured:boolean}>(`/teams/${team}/provider`);if(!provider?.key_configured)throw new Error(tr("Heutigen Teamprovider autorisieren"));await api(`/team-schedules/${team}/${project}`,"POST",{id:null,expected_revision:0,idempotency_key:key(c.id,"schedule"),name:c.source.name,timezone:c.source.timezone??"UTC",recurrence,template:{order_id:order.id,prompt:currentPrompt(c),project_revision:current.revision,order_revision:order.revision,provider_revision:provider.revision},paused:true,grant_execution:true});}else{const p=providers.find(p=>p.id===provider&&p.credential_ready);if(!p)throw new Error(tr("Heutigen Provider wählen"));await api(base+"/schedules","POST",{id:null,expected_revision:0,name:c.source.name,timezone:c.source.timezone??"UTC",recurrence,template:{order_id:order.id,prompt:currentPrompt(c),provider_id:p.id,provider_revision:p.revision,budgets:defaultRunBudgets},paused:true});}setNotice(tr("Neuer pausierter V2-Zeitplan mit heutiger Freigabe gespeichert. Aktivierung erfolgt im Zeitplanbereich."));});}
 if(!items.length&&!error)return null;
 return <section><h3>{tr("Übernommene Definitionen und Herkunft")}</h3>{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}{notice&&<p role="status">{notice}</p>}<button disabled={busy} onClick={()=>void attempt(refresh)}>{tr("Aktualisieren")}</button>{!team&&providers.length>0&&<label>{tr("Aktueller Provider für neue Definitionen")}<select value={provider} onChange={e=>setProvider(e.target.value)}>{providers.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label>}{items.map(c=><article className="product-item" key={c.id}><strong>{c.source.name}</strong><small>{c.source.kind}{tr(" · Original ")}{c.source.source_id}{tr(" · Version ")}{c.source.source_revision} · {c.state}</small>{c.document_id&&<p>{tr("Als aktuelles V2-Dokument ")}{c.document_id}{tr(" verfügbar.")}</p>}<details><summary>{tr("Herkunft und Versionen")}</summary><p>{c.source.description}</p><pre>{c.source.instructions}</pre>{c.source.versions.map(v=><details key={v.source_id}><summary>{tr("Originalversion ")}{v.revision} · {v.summary}</summary><pre>{v.content}</pre></details>)}</details>{c.source.kind==="provider"&&(team?<p>{tr("Teamprovider im Bereich „Team & Provider“ mit aktuellen Teamrechten und neuem eigenen LiteLLM-Schlüssel konfigurieren.")}</p>:<ImportedProviderSetup configuration={c} client={client} disabled={busy} complete={async configured=>{setProviders(await client.providers());setProvider(configured.id);}}/>)}{!team&&["persona","workflow"].includes(c.source.kind)&&<HistoricalLibraryAdoption key={JSON.stringify([client.base,client.token,project,c.id])} client={client} project={project} configuration={c} providers={providers} disabled={busy} workflowOverride={plans[c.id]}/>}{["workflow","schedule"].includes(c.source.kind)&&<><label>{tr("Neuer Auftragstext")}<textarea rows={3} value={currentPrompt(c)} onChange={e=>setPrompt({...prompt,[c.id]:e.target.value})}/></label>{c.source.kind==="workflow"&&<label>{tr("V2-Agenten und Abhängigkeiten (ohne alte Tools oder Modellrechte)")}<textarea rows={8} value={plans[c.id]??JSON.stringify(c.source.workflow,null,2)} onChange={e=>setPlans({...plans,[c.id]:e.target.value})}/></label>}{c.source.kind==="schedule"&&<><p>{tr("Historischer Ausdruck: ")}{c.source.schedule_expression} · {c.source.timezone}</p><label>{tr("Ausdrücklich gewähltes V2-Zeitmuster")}<textarea rows={3} value={recurrences[c.id]??JSON.stringify(cron(c.source.schedule_expression),null,2)} onChange={e=>setRecurrences({...recurrences,[c.id]:e.target.value})}/></label></>}<div className="actions"><button disabled={busy||!!prepared[c.id]} onClick={()=>void prepare(c)}>{tr("Neuen Auftrag vorbereiten")}</button>{c.source.kind==="workflow"&&(team?<button disabled={busy||!prepared[c.id]} onClick={()=>void start(c)}>{tr("Mit heutigen Rechten neu starten")}</button>:null)}{c.source.kind==="schedule"&&<button disabled={busy||!prepared[c.id]} onClick={()=>void schedule(c)}>{tr("Heutige Ausführung freigeben und neuen Plan pausiert speichern")}</button>}</div></>}</article>)}</section>;
}
