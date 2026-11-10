import {useEffect,useRef,useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {Client,RequestError,decode,type ConfigureMcpServer,type McpServer,type McpToolSelection,type McpAttempt} from "@cowork/client";
import type {Key} from "./i18n";
import {productRequest} from "./productApi";
import {nativeMcpAvailable,stdioFactories,configureStdio,saveMcpCredential,disconnectStdio,mcpCredentialStatus,serverMcpCapabilities,serverStdioFactories,configureServerStdio,saveServerMcpCredential,serverMcpCredentialStatus,disconnectServerStdio,parseMcpEnvironment,type McpDiscovery} from "./mcpBridge";
import {useAgentDefaults} from "./AgentDefaultsPanel";
import {useView} from "./view";
import {definitiveRequestRejection} from "./requestJournal";
import {NativeMcpOAuthPanel,McpTaskPanel} from "./McpOAuthTasks";
type T=(key:Key)=>string;

export function McpTools({client,project,selected,setSelected,t,mode="select"}:{client:Client,project:string,selected:McpToolSelection[],setSelected:(value:McpToolSelection[])=>void,t:T,mode?:"select"|"manage"}){
  const language=useView(v=>v.language);const manage=mode==="manage";
  const qc=useQueryClient();
  const servers=useQuery({queryKey:["mcp-servers",client.base,project],queryFn:()=>client.mcpServers(project),retry:false});
  const [editing,setEditing]=useState<McpServer|null|undefined>();
  const stale=selected.some(s=>!servers.data?.some(r=>r.id===s.server_id&&r.revision===s.server_revision&&r.configuration.enabled&&r.configuration.selected_tools.includes(s.tool_name)));
  return <section className="mcp-tools"><p>{manage?(language==="de"?"Verbinde MCP-Server und lege fest, welche Werkzeuge im Projekt verfügbar sind.":"Connect MCP servers and configure the tools available to this project."):t("mcpSelectionHint")}</p>
    <p className="muted">{manage?(language==="de"?"Die Werkzeugauswahl für einen konkreten Lauf erfolgt im Chat oder Workflow.":"Select tools for an individual run in the chat or workflow."):t("mcpScopeHint")}</p>
    {servers.isPending&&<p role="status">{t("loading")}</p>}
    {servers.isError&&<p role="alert">{t("mcpLoadFailed")}</p>}
    <button type="button" onClick={()=>void servers.refetch()}>{t("mcpRefresh")}</button>
    {servers.data?.map(server=><section className="mcp-server" key={server.id}>
      <header><h3>{server.configuration.name}</h3>{server.configuration.catalog.some(tool=>tool&&typeof tool==="object"&&!Array.isArray(tool)&&(tool as Record<string,unknown>)._meta&&((tool as Record<string,unknown>)._meta as Record<string,unknown>)["io.localai.connector"]===true)?<span className="muted">{language==="de"?"In Connectors verwalten":"Manage in Connectors"}</span>:<button type="button" onClick={()=>setEditing(server)}>{t("mcpEdit")}</button>}</header>
      <p>{server.configuration.endpoint}</p><p>{t(server.configuration.enabled?"mcpEnabled":"mcpDisabled")}</p>
      {!server.configuration.endpoint.includes("/native-mcp/")&&<NativeMcpOAuthPanel client={client} project={project} server={server} changed={()=>servers.refetch()}/>}
      {manage?<ul className="settings-tool-list">{server.configuration.selected_tools.map(name=><li key={name}>{name}</li>)}</ul>:<div className="folder-options">{server.configuration.selected_tools.map(name=><label key={name}>
        <input type="checkbox" disabled={!server.configuration.enabled||!!editing} checked={selected.some(s=>s.server_id===server.id&&s.server_revision===server.revision&&s.tool_name===name)}
          onChange={e=>setSelected([...selected.filter(s=>s.server_id!==server.id||s.tool_name!==name),...(e.target.checked?[{server_id:server.id,server_revision:server.revision,tool_name:name}]:[])])}/><span>{name}</span>
      </label>)}</div>}
      <details><summary>{t("mcpCatalogDetails")}</summary><pre>{JSON.stringify(server.configuration.catalog,null,2)}</pre></details>
    </section>)}
    {!servers.isPending&&!servers.data?.length&&<p>{t("mcpNoServers")}</p>}
    {stale&&servers.data&&<p role="alert">{t("mcpSelectionChanged")}</p>}
    <div className="actions">{!manage&&<button type="button" onClick={()=>setSelected([])}>{t("mcpClear")}</button>}<button type="button" onClick={()=>setEditing(null)}>{t("mcpAdd")}</button></div>
    {editing!==undefined&&<McpServerForm key={editing?.id??"new"} existing={editing} client={client} project={project} t={t} done={async()=>{setEditing(undefined);await qc.invalidateQueries({queryKey:["mcp-servers",client.base,project]});}}/>}
  </section>;
}

function McpServerForm({existing,client,project,t,done}:{existing:McpServer|null,client:Client,project:string,t:T,done:()=>Promise<void>}){
  const language=useView(v=>v.language);
  const preferences=useAgentDefaults(client),environmentEditor=preferences.data?.value.mcp_env_editor_enabled===true,manualCatalog=preferences.data?.value.mcp_allow_manual_import===true;
  const credentialPending=useRef<{project_id:string,server_id:string,endpoint:string,bearer:string,expected_revision:number}|null>(null);
  const [serverId]=useState(existing?.id??crypto.randomUUID());
  const local=nativeMcpAvailable(client);
  const capabilities=useQuery({queryKey:["mcp-capabilities",client.base],queryFn:()=>serverMcpCapabilities(client),enabled:!local,retry:false});
  const canStdio=local||capabilities.data?.stdio===true,canCredential=local||capabilities.data?.http===true;
  const factories=useQuery({queryKey:["stdio-factories",client.base,project],queryFn:()=>local?stdioFactories(project):serverStdioFactories(client,project),enabled:canStdio,retry:false});
  const factory=factories.data?.find(f=>f.server_id===serverId);
  const [transport,setTransport]=useState(existing?.configuration.endpoint.includes("/native-mcp/")?"stdio":"http");
  const [program,setProgram]=useState("node"),[argumentsText,setArgumentsText]=useState("[]"),[bearer,setBearer]=useState("");
  const [cwd,setCwd]=useState("."),[environmentText,setEnvironmentText]=useState("{}"),[replaceEnvironment,setReplaceEnvironment]=useState(false);
  useEffect(()=>{if(!environmentEditor){setReplaceEnvironment(false);setEnvironmentText("{}");}},[environmentEditor]);
  const hydratedFactory=useRef<number|undefined>(undefined),[stdioRevision,setStdioRevision]=useState(0),[stdioDirty,setStdioDirty]=useState(false);
  useEffect(()=>{if(factory&&!stdioDirty&&factory.revision!==hydratedFactory.current){hydratedFactory.current=factory.revision;setStdioRevision(factory.revision);setProgram(factory.program);setArgumentsText(JSON.stringify(factory.arguments,null,2));setCwd(factory.cwd??".");setReplaceEnvironment(false);setEnvironmentText("{}");}},[factory,stdioDirty]);
  const availability=useQuery({queryKey:["toolbox-mcp-programs",client.base],queryFn:()=>productRequest<{programs:string[]}>(client,"/toolbox"),enabled:local,retry:false});
  const [credential,setCredential]=useState({credential_reference:existing?.configuration.credential_reference??null,credential_revision:existing?.configuration.credential_revision??0});
  const [name,setName]=useState(existing?.configuration.name??"");
  const [endpoint,setEndpoint]=useState(existing?.configuration.endpoint??"");
  const [catalog,setCatalog]=useState(JSON.stringify(existing?.configuration.catalog??[],null,2));
  const [tools,setTools]=useState(existing?.configuration.selected_tools??[]);
  const [enabled,setEnabled]=useState(existing?.configuration.enabled??true);
  const [pending,setPending]=useState<ConfigureMcpServer|null>(null);
  const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();
  let definitions:{name:string}[]=[];
  try{const parsed:unknown=JSON.parse(catalog);if(Array.isArray(parsed)&&parsed.every(d=>d&&typeof d==="object"&&typeof d.name==="string"))definitions=parsed;}catch{/* Server validates the full schema on save. */}
  async function discover(){if(busy||!(transport==="http"?canCredential:canStdio))return;setBusy(true);setError(undefined);try{
    let result:McpDiscovery|null;
    if(transport==="stdio"){
      const args:unknown=JSON.parse(argumentsText);if(!Array.isArray(args)||!args.every(a=>typeof a==="string"))throw new Error(t("mcpArguments"));
      if(factories.isPending||factories.isError)throw new Error(t("mcpLoadFailed"));
      const input={project_id:project,server_id:serverId,program,arguments:args,cwd:cwd.trim()||null,environment:replaceEnvironment?parseMcpEnvironment(environmentText):null,expected_revision:stdioRevision};
      result=local?await configureStdio(input,language):await configureServerStdio(client,input);
      if(!result)return;setEndpoint(result.endpoint!);setCredential({credential_reference:result.credential_reference!,credential_revision:result.credential_revision!});setStdioDirty(false);setEnvironmentText("{}");setReplaceEnvironment(false);
    }else{
      let saved=credential;
      if(bearer||credentialPending.current){if(!canCredential)throw new Error(t("mcpNativeRequired"));const original=credentialPending.current??{project_id:project,server_id:serverId,endpoint,bearer,expected_revision:credential.credential_revision};credentialPending.current=original;saved=local?await saveMcpCredential(original):await saveServerMcpCredential(client,original);credentialPending.current=null;setCredential(saved);setBearer("");}
      result=local?await productRequest<McpDiscovery>(client,`/projects/${project}/mcp-discovery`,"POST",{endpoint,...(saved.credential_reference?{server_id:serverId,credential_revision:saved.credential_revision}:{})}):await productRequest<McpDiscovery>(client,"/mcp/discovery","POST",{project_id:project,server_id:serverId,endpoint,credential_reference:saved.credential_reference,credential_revision:saved.credential_revision});
    }
    setCatalog(JSON.stringify(result.catalog,null,2));const names=result.catalog.flatMap(d=>d&&typeof d==="object"&&"name" in d&&typeof d.name==="string"?[d.name]:[]);setTools(previous=>previous.filter(n=>names.includes(n)));
  }catch(e){setError(e);if(e instanceof RequestError&&["invalid_request","forbidden","conflict","revision_conflict","not_found"].includes(e.details.code))credentialPending.current=null;}finally{if(transport==="stdio")await factories.refetch();setBusy(false);}}
  async function save(event:FormEvent){event.preventDefault();if(busy)return;setError(undefined);setBusy(true);
    try{
      if(bearer)throw new Error(t("mcpDiscover"));
      if(transport==="stdio"&&stdioDirty)throw new Error(language==="de"?"Geänderte stdio-Parameter zuerst verbinden und den Katalog neu laden.":"Connect edited stdio parameters and reload the catalog before saving.");
      const request=pending??decode("ConfigureMcpServer",{server_id:serverId,base_revision:existing?.revision??0,idempotency_key:crypto.randomUUID(),configuration:{name,endpoint,catalog:JSON.parse(catalog),selected_tools:tools,enabled,...credential}});
      setPending(request);
      await client.configureMcpServer(project,request);
      await done();
    }catch(e){setError(e);if(e instanceof RequestError&&definitiveRequestRejection(e.status))setPending(null);}finally{setBusy(false);}
  }
  return <form className="mcp-config" onSubmit={save}><h3>{t(existing?"mcpEdit":"mcpAdd")}</h3><p>{t("mcpImportHint")}</p>
    <fieldset disabled={busy||!!pending}>
      <label>{t("mcpName")}<input value={name} onChange={e=>setName(e.target.value)} maxLength={200} required/></label>
      <label>{t("mcpTransport")}<select value={transport} disabled={!!existing} onChange={e=>setTransport(e.target.value)}><option value="http">Streamable HTTP</option><option value="stdio" disabled={!canStdio}>stdio</option></select></label>
      {transport==="http"?<><label>{t("mcpBearer")}<input type="password" value={bearer} disabled={!canCredential||!!credentialPending.current} onChange={e=>setBearer(e.target.value)} autoComplete="off" maxLength={4096}/></label>{canCredential&&<button type="button" onClick={()=>{setBusy(true);setError(undefined);void(local?mcpCredentialStatus(project,serverId):serverMcpCredentialStatus(client,project,serverId)).then(saved=>{if(new URL(saved.endpoint).toString()!==new URL(endpoint).toString())throw new Error(t("mcpCredentialMismatch"));setCredential({credential_reference:saved.credential_reference,credential_revision:saved.credential_revision});credentialPending.current=null;setBearer("");}).catch(setError).finally(()=>setBusy(false));}}>{t("mcpCredentialRecover")}</button>}<button type="button" disabled={!canCredential||busy} onClick={()=>void discover()}>{t("mcpDiscover")}</button></>:<><label>{t("mcpProgram")}<select value={program} disabled={factories.isPending} onChange={e=>{setProgram(e.target.value);setStdioDirty(true);}}>{(local?(availability.data?.programs??["node","python"]):(capabilities.data?.programs??[])).map(p=><option key={p}>{p}</option>)}</select></label><label>{t("mcpArguments")}<textarea value={argumentsText} disabled={factories.isPending} onChange={e=>{setArgumentsText(e.target.value);setStdioDirty(true);}} rows={3} maxLength={32768}/></label>
        <label>{language==="de"?"Arbeitsverzeichnis":"Working directory"}<input value={cwd} maxLength={4096} onChange={event=>{setCwd(event.target.value);setStdioDirty(true);}} placeholder="."/></label><p className="muted">{local?(language==="de"?"Absolute lokale Pfade werden im nativen Bestätigungsdialog angezeigt.":"Absolute local paths are displayed in the native confirmation dialog."):(language==="de"?"Relative Pfade bleiben im verwalteten Arbeitsverzeichnis dieses MCP-Servers.":"Relative paths remain inside this MCP server's managed workspace.")}</p>
        {!!factory&&Object.keys(factory.environment??{}).length>0&&<p>{language==="de"?"Gespeicherte Variablen: ":"Stored variables: "}{Object.keys(factory.environment??{}).join(", ")}</p>}
        {environmentEditor&&<label className="settings-checkbox"><input type="checkbox" checked={replaceEnvironment} onChange={event=>{setReplaceEnvironment(event.target.checked);setStdioDirty(true);}}/>{language==="de"?"Umgebungsvariablen ersetzen":"Replace environment variables"}</label>}{environmentEditor&&replaceEnvironment&&<label>{language==="de"?"Variablen als JSON-Objekt":"Environment as a JSON object"}<textarea value={environmentText} rows={5} spellCheck={false} autoComplete="off" maxLength={16384} placeholder={'{"API_KEY":"…"}'} onChange={event=>{setEnvironmentText(event.target.value);setStdioDirty(true);}}/></label>}<p className="muted">{language==="de"?"Werte werden verschlüsselt gespeichert. Ein leeres Objekt entfernt die bisherigen Variablen; ohne Ersetzen bleiben sie erhalten.":"Values are stored encrypted. An empty object removes previous variables; leave replacement off to retain them."}</p>
        <button type="button" disabled={!canStdio} onClick={()=>void discover()}>{t("mcpConnectStdio")}</button>{factory&&stdioDirty&&<button type="button" onClick={()=>{hydratedFactory.current=undefined;setStdioDirty(false);setEnvironmentText("{}");setReplaceEnvironment(false);}}>{language==="de"?"Gespeicherte Parameter wieder laden":"Reload saved parameters"}</button>}{factory?.enabled&&<button type="button" onClick={()=>{setBusy(true);void(local?disconnectStdio(project,serverId,factory.revision):disconnectServerStdio(client,project,serverId,factory.revision)).then(()=>factories.refetch()).catch(setError).finally(()=>setBusy(false));}}>{t("mcpDisconnectStdio")}</button>}</>}
      <p className="muted">{t("mcpDiscoveryHint")}</p>{!local&&!capabilities.isPending&&!canCredential&&<p role="status">{language==="de"?"Diese Verbindung erlaubt keine MCP-Einrichtung. Öffne den lokalen Arbeitsplatz in der Desktop-App oder verbinde einen Server mit aktivierter MCP-Einrichtung.":"This connection does not allow MCP setup. Open the local workspace in the desktop application or connect to a server with MCP setup enabled."}</p>}{!local&&capabilities.isError&&<p role="alert">{language==="de"?"Die Verbindungsmöglichkeiten dieses Servers konnten nicht geladen werden.":"This server's connection capabilities could not be loaded."}</p>}
      <label>{t("endpoint")}<input value={endpoint} readOnly={transport==="stdio"} disabled={!!credentialPending.current} onChange={e=>setEndpoint(e.target.value)} type="url" required/></label>
      <details><summary>{t("mcpCatalogDetails")}</summary><label>{t("mcpCatalogJson")}<textarea readOnly={!manualCatalog} value={catalog} onChange={e=>{if(manualCatalog)setCatalog(e.target.value);}} rows={8} maxLength={900000} spellCheck={false}/></label></details>
      <p>{t("mcpProjectTools")}</p><div className="folder-options">{definitions.map((definition,i)=><label key={i}><input type="checkbox" checked={tools.includes(definition.name)} onChange={e=>setTools(e.target.checked?[...tools.filter(n=>n!==definition.name),definition.name]:tools.filter(n=>n!==definition.name))}/><span>{definition.name}</span></label>)}</div>
      <label><input type="checkbox" checked={enabled} onChange={e=>setEnabled(e.target.checked)}/>{t("mcpEnabled")}</label>
    </fieldset>
    {(pending||credentialPending.current)&&<p role="status">{t("mcpSavePending")}</p>}
    {!!error&&<p role="alert">{t(error instanceof RequestError&&error.status===409?"mcpConfigConflict":"mcpSaveFailed")} {error instanceof Error?error.message:String(error)}</p>}
    <div className="actions"><button type="button" disabled={busy} onClick={()=>void done()}>{t("close")}</button><button type="submit" className="primary" disabled={busy}>{t(pending?"mcpRetry":"mcpSave")}</button></div>
  </form>;
}

export function McpAttempts({attempts,interrupted,t,client,refresh}:{attempts:McpAttempt[],interrupted:boolean,t:T,client:Client,refresh:()=>Promise<unknown>}){
  if(!attempts.length)return null;
  return <section className="mcp-attempts"><h3>{t("mcpExecution")}</h3>
    <McpTaskPanel client={client} run={attempts[0]!.run_id}/>
    {attempts.map(a=><div key={a.id}><p>{String((a.binding.definition as Record<string,unknown>).name)} · {t(a.state.state==="completed"||a.state.state==="reconciled_completed"?"completed":a.state.state==="dispatch_committed"&&!interrupted?"mcpDispatch":a.state.state==="reconciled_not_executed"?"mcpNotExecuted":"mcpUncertain")}</p><p>{a.binding.endpoint}</p>
      {(a.state.state==="outcome_uncertain"||a.state.state==="dispatch_committed"&&interrupted)&&<p>{t("mcpUncertainHint")}</p>}
      <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(a,null,2)}</pre></details>
      {interrupted&&(a.state.state==="outcome_uncertain"||a.state.state==="dispatch_committed")&&<McpReceiptReview attempt={a} client={client} refresh={refresh} t={t}/>}
    </div>)}
  </section>;
}
function McpReceiptReview({attempt:a,client,refresh,t}:{attempt:McpAttempt,client:Client,refresh:()=>Promise<unknown>,t:T}){
  const [receipt,setReceipt]=useState(""),[source,setSource]=useState(""),[outcome,setOutcome]=useState("completed"),[verified,setVerified]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState<unknown>(),[pending,setPending]=useState<Record<string,unknown>|null>(null);
  async function submit(e:FormEvent){e.preventDefault();setBusy(true);setError(undefined);try{const body=pending??{expected_revision:a.revision,request_sha256:a.request_sha256,review:{owner_verified:verified,source,outcome,server_id:a.binding.server_id,rpc_request_id:a.rpc_request_id,request_sha256:a.request_sha256,receipt:JSON.parse(receipt)}};setPending(body);await productRequest(client,`/runs/${a.run_id}/mcp-attempts/${encodeURIComponent(a.tool.id)}/reconcile`,"POST",body);await refresh();}catch(e){setError(e);if(/\((400|403|404|409)\)/.test(String(e)))setPending(null);}finally{setBusy(false);}}
  return <details><summary>{t("mcpReview")}</summary><p>{t("mcpReviewHint")}</p><pre>{JSON.stringify({server_id:a.binding.server_id,rpc_request_id:a.rpc_request_id,request_sha256:a.request_sha256},null,2)}</pre><form onSubmit={submit}><fieldset disabled={busy||!!pending}><label>{t("mcpOutcome")}<select value={outcome} onChange={e=>setOutcome(e.target.value)}><option value="completed">{t("completed")}</option><option value="not_executed">{t("mcpNotExecuted")}</option></select></label><label>{t("mcpEvidenceSource")}<input value={source} onChange={e=>setSource(e.target.value)} required maxLength={2048}/></label><label>{t("mcpReceipt")}<textarea value={receipt} onChange={e=>setReceipt(e.target.value)} required rows={8} maxLength={900000}/></label><label><input type="checkbox" checked={verified} onChange={e=>setVerified(e.target.checked)} required/>{t("mcpReviewConfirmed")}</label></fieldset>{!!error&&<p role="alert">{error instanceof Error?error.message:String(error)}</p>}<button disabled={busy||!verified}>{t(pending?"mcpRetry":"mcpReviewSave")}</button></form></details>;
}
