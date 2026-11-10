import { useCallback, useEffect, useId, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { QueryClient, QueryClientProvider, useQuery, useQueryClient } from "@tanstack/react-query";
import { Client, RequestError, type WorkflowOperation, type AccessMode, type Order, type Artifact, type ProviderProfile, type RunRecord, type CreateRun, type Message, type BranchOrder, type FolderGrant, type RestoreFile, type CreateRestoreRun,type FileUpload,type ImportFile,type CreateImportRun } from "@cowork/client";
import { ArrowUp, Plus, Folder, MessageSquare, PanelRight, Menu, X, Settings2, ShieldCheck, Monitor, ChevronRight, FileText, Download, Sun, Moon, Search, CircleCheck, Square, RotateCcw, Plug, Activity, Inbox, GitBranch, User, Globe, SquareTerminal, SlidersHorizontal, CodeXml, Bot, Box, Gauge, Braces, Cpu, ChevronLeft, BookOpen, Command, type LucideIcon } from "lucide-react";
import { ActivityPanel, SearchPanel } from "./WorkPanels";
import { FileChanges } from "./FileChanges";
import {RestoreExecutionPlanReview} from "./RestoreExecutionPlanReview";
import {ImportExecutionPlanReview} from "./ImportExecutionPlanReview";
import { DocumentToolSummary, isDocumentTool } from "./DocumentToolSummary";
import { ToolPreviews } from "./ToolPreviews";
import { useDocumentAutoSetup } from "./documentAutoSetup";
import { WorkingFiles } from "./WorkingFiles";
import {ManagedProcessRecoveryPanel,managedProcessRecoveryAvailable} from "./ManagedProcessRecoveryPanel";
import {ImportFileForm} from "./FileUploads";
import {saveCurrentDraft} from "./fileActionRevision";
import {NativeFolders} from "./NativeFolders";
import {NativeSecrets} from "./NativeSecrets";
import {McpTools,McpAttempts} from "./McpTools";
import {defaultRunBudgets} from "@cowork/client";
import {RunBudgets,RunBudgetStatus} from "./RunBudgets";
import {PersonalMoneyStatus} from "./PersonalMoneyStatus";
import type {SettingsSection,SettingsTabRequest} from "./SettingsWorkspace";
import {SettingsWorkspace,OfficeWorkbench,DeveloperWorkspace,TerminalDock,WorkflowLibrary,TeamPanel} from "./DeferredWorkspaces";
import {finishedTransitions} from "./desktopBehavior";
import {appearanceScope,useAppearancePreferences,applyAppearancePreferences} from "./appearancePreferences";
import {AgentLibrarySelector,useAgentLibrary} from "./AgentLibrary";
import {effectiveAgent,effectivePersonality,type EffectiveAgentConfig,type Personality,type AgentTemplate} from "./libraryApi";
import {AgentDefaultsPanel,useAgentDefaults} from "./AgentDefaultsPanel";
import {useChatConfiguration,chatConfigurationEqual,chatWorkflowSelection,exactChatProvider,copiedChatAgent} from "./chatConfiguration";
import {ChatMessageBody,chatTranscriptText} from "./ChatMessageBody";
import {ArtifactWorkbench} from "./ArtifactWorkbench";
import {announceCompletion} from "./desktopEffects";
import {materializeCodexModelProfile} from "./codexModelApi";
import {TaskWorkspace} from "./TaskWorkspace";
import {ProjectSettingsPanel} from "./ProjectSettingsPanel";
import {PersonalKnowledgeBase} from "./ProjectKnowledgeBase";
import {ChatManagementDialog} from "./ChatManagementDialog";
import {ArchivedChatsPanel} from "./ArchivedChatsPanel";
import {independentChats,visibleProjects} from "./chatScope";
import {ChatDeletionProvider,ChatDeletionNotice} from "./ChatDeletion";
import {PersistentRequestLatch,definitiveRequestRejection} from "./requestJournal";
import {ExecutionPlanReview} from "./ExecutionPlanReview";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {answerCanRegenerate,prepareRegenerationReview,singleStartJournalKey,decodeRegenerationIntent,workflowRegenerationIntent,type RegenerationIntent} from "./regenerationReview";
import {decode} from "@cowork/client";
import {CommandPalette,useCommandShortcut,workspaceCommands,parseWorkspaceSlashCommand,runWorkspaceSlashCommand,SlashCommandHelp,type WorkspaceCommandActions,type WorkspaceSlashCommand} from "./CommandPalette";
import {EnvironmentPicker,type EnvironmentReference} from "./EnvironmentPanel";
import {ModelRegistryPicker} from "./ModelRegistryPicker";
import {ContextPanel} from "./ContextPanel";
import {ToolRuntimeSetup} from "./ToolRuntimeSetup";
import {DesktopUpdates} from "./DesktopUpdates";
import {productRequest} from "./productApi";
import {ImportedHistory} from "./ImportedHistory";
import {connectionEndpoint, nativeConnectionAllowed, preferNativeConnection, storedConnection, type ConnectionSettings} from "./connectionSettings";
import {forgetWorkspace, loadWorkspaceProfiles, rememberWorkspace, updateWorkspace, workspaceId, workspaceToken, type WorkspaceProfile} from "./workspaceProfiles";
import {WorkspaceConnectionEditor, WorkspaceSwitcher, type WorkspaceChanges} from "./WorkspaceSwitcher";
import {SpeechInputButton,appendTranscript} from "./SpeechInput";
import {StructuredOutputEditor,FrozenOutputPreview,type OutputContract} from "./StructuredOutputEditor";
import {WorkflowRunPanel,workflowCanResume} from "./WorkflowRunPanel";
import {findWorkflowSelection,workflowSelection} from "./workflowUi";
import {WorkflowDataValueEditor} from "./WorkflowDataflowEditor";
import {WorkflowDocumentSources} from "./WorkflowDocumentSources";
import {defaultDataValue,workflowInputForStart} from "./workflowDataflow";
import {WorkflowVersionPreview} from "./WorkflowVersionPreview";
import {useWorkflowRecovery,WorkflowOperationPanel} from "./workflowJournal";
import {canCompactWorkflowComposer,ComposerDisclosure,confirmedWorkflowStart,confirmedSnapshotWorkflowStart} from "./composerLayout";
import {deliverDownload,deliveryStatusKey} from "./download";
import { useView } from "./view";
import { de,en,type Key } from "./i18n";
import "./styles.css";
import "./workspaceNavigation.css";
const queries=new QueryClient({defaultOptions:{queries:{retry:1,refetchOnWindowFocus:true},mutations:{retry:false}}});
type T=(key:Key)=>string;
const terminal=(run:RunRecord)=>["completed","failed","canceled","expired"].includes(run.state);
function useTranslation():T { const language=useView(v=>v.language);return key=>(language==="de"?de:en)[key]; }
function ErrorNotice({error,t}:{error:unknown,t:T}) {
  if(!error)return null;
  const code=error instanceof RequestError?error.details.code:error instanceof Error&&error.message in de?error.message:"error";
  return <div className="error" role="alert"><strong>{t(code in de?code as Key:"error")}</strong>
    <details><summary>{t("problemDetails")}</summary><p>{error instanceof Error?error.message:String(error)}</p>
    {error instanceof RequestError&&<code>{error.details.correlation_id}</code>}</details></div>;
}
function Dialog({title,onClose,children,className,closeBlocked=false}:{title:string,onClose:()=>void,children:ReactNode,className?:string,closeBlocked?:boolean}) {
  const ref=useRef<HTMLDialogElement>(null);
  const titleId=useId();
  useEffect(()=>{
    const d=ref.current!;const previous=document.activeElement;
    d.showModal();d.querySelector<HTMLInputElement>("input")?.focus();
    return()=>{d.close();if(previous instanceof HTMLElement&&previous.isConnected)previous.focus({preventScroll:true});};
  },[]);
  return <dialog ref={ref} className={className} aria-labelledby={titleId} onCancel={event=>{if(closeBlocked)event.preventDefault();}} onClose={()=>{if(ref.current&&!ref.current.open&&!closeBlocked)onClose();}}>
    <header><h2 id={titleId}>{title}</h2><button className="icon" disabled={closeBlocked} onClick={onClose} aria-label={useTranslation()("close")}><X size={20}/></button></header>{children}
  </dialog>;
}
function useMedia(query:string){const [matches,set]=useState(()=>window.matchMedia(query).matches);
  useEffect(()=>{const m=window.matchMedia(query);const update=()=>set(m.matches);m.addEventListener("change",update);update();return()=>m.removeEventListener("change",update);},[query]);return matches;}
export default function App(){
  return <QueryClientProvider client={queries}><Connection /></QueryClientProvider>;
}
function Connection(){
  const t=useTranslation(); const language=useView(v=>v.language);const theme=useView(v=>v.theme);
  const [connection,setConnection]=useState(()=>storedConnection(sessionStorage));
  const [workspaces,setWorkspaces]=useState(()=>loadWorkspaceProfiles(localStorage,sessionStorage));
  const [adding,setAdding]=useState(false);
  const [reconnecting,setReconnecting]=useState<WorkspaceProfile|null>(null);
  const [editing,setEditing]=useState<WorkspaceProfile|null>(null);
  const [workspaceVersion,setWorkspaceVersion]=useState(0);
  const [teamConnection,setTeamConnection]=useState<string|null>(null);
  const connectionGeneration=useRef(0);
  const [ready,setReady]=useState(false);const [error,setError]=useState<unknown>(); const [busy,setBusy]=useState(false);
  const client=useMemo(()=>new Client(connection.token,connection.base),[connection]);
  // In-flight callbacks from the previous server must retain their own cache.
  const workspaceQueries=useMemo(()=>new QueryClient({defaultOptions:{queries:{retry:1,refetchOnWindowFocus:true},mutations:{retry:false}}}),[client]);
  async function verifyConnection(next:ConnectionSettings,name:string,generation:number,saved=workspaces) {
    await new Client(next.token,next.base).health();
    if(generation!==connectionGeneration.current)return;
    const profiles=rememberWorkspace(localStorage,sessionStorage,saved,next,name);
    sessionStorage.setItem("cowork-v2-token",next.token);
    sessionStorage.setItem("cowork-v2-endpoint",next.base);
    sessionStorage.setItem("cowork-v2-connection-kind",next.kind);
    workspaceQueries.clear();
    if(next.base!==connection.base||next.kind!==connection.kind||next.token!==connection.token)useView.getState().selectLocation("","");
    setWorkspaces(profiles);setConnection({...next});setWorkspaceVersion(value=>value+1);setReady(true);setAdding(false);setReconnecting(null);setError(undefined);
  }
  async function connect(next:ConnectionSettings,name="") {
    const generation=++connectionGeneration.current;setBusy(true);setError(undefined);
    try {await verifyConnection(next,name,generation);}
    catch(error){if(generation===connectionGeneration.current)setError(error);}
    finally{if(generation===connectionGeneration.current)setBusy(false);}
  }
  async function openLocalWorkspace(name="",saved=workspaces) {
    const generation=++connectionGeneration.current;setBusy(true);setError(undefined);
    try {
      const {invoke}=await import("@tauri-apps/api/core");
      const info=await invoke<{token:string,endpoint:string}>("connection_info");
      if(generation===connectionGeneration.current)await verifyConnection({token:info.token,base:connectionEndpoint(info.endpoint),kind:"native"},name,generation,saved);
    } catch(error) {if(generation===connectionGeneration.current)setError(error);}
    finally {if(generation===connectionGeneration.current)setBusy(false);}
  }
  function closeConnectionDialog(){connectionGeneration.current++;setAdding(false);setReconnecting(null);setBusy(false);setError(undefined);}
  function closeEditDialog(){connectionGeneration.current++;setEditing(null);setBusy(false);setError(undefined);}
  async function saveWorkspace(profile:WorkspaceProfile,changes:WorkspaceChanges) {
    const generation=++connectionGeneration.current;setBusy(true);setError(undefined);
    try {
      const base=profile.kind==="native"?profile.base:connectionEndpoint(changes.endpoint),moved=workspaceId({base,kind:profile.kind})!==profile.id;
      const token=changes.token||(moved?"":workspaceToken(sessionStorage,profile)),active=profile.id===workspaceId(connection);
      if(changes.token)await new Client(token,base).health();
      if(generation!==connectionGeneration.current)return;
      setWorkspaces(updateWorkspace(localStorage,sessionStorage,workspaces,profile,{name:changes.name,base,token:changes.token}));
      if(active&&(moved||changes.token)){
        sessionStorage.setItem("cowork-v2-token",token);sessionStorage.setItem("cowork-v2-endpoint",base);sessionStorage.setItem("cowork-v2-connection-kind",profile.kind);
        workspaceQueries.clear();if(moved)useView.getState().selectLocation("","");
        setConnection({token,base,kind:profile.kind});setWorkspaceVersion(value=>value+1);
      }
      setEditing(null);
    } catch(error) {if(generation===connectionGeneration.current)setError(error);}
    finally {if(generation===connectionGeneration.current)setBusy(false);}
  }
  function removeWorkspace(profile:WorkspaceProfile){
    connectionGeneration.current++;
    const remaining=forgetWorkspace(localStorage,sessionStorage,workspaces,profile.id);
    setWorkspaces(remaining);setEditing(null);setBusy(false);setError(undefined);
    if(profile.id!==workspaceId(connection))return;
    workspaceQueries.clear();useView.getState().selectLocation("","");setConnection({token:"",base:"",kind:"server"});setReady(false);
    if("__TAURI_INTERNALS__" in window)void openLocalWorkspace("",remaining);
  }
  function selectWorkspace(profile:WorkspaceProfile){
    if(profile.kind==="native"){void openLocalWorkspace(profile.name);return;}
    const token=workspaceToken(sessionStorage,profile);
    if(token){void connect({...profile,token},profile.name);return;}
    setReconnecting(profile);setAdding(true);setError(undefined);
  }
  useEffect(()=>{document.documentElement.lang=language;document.documentElement.dataset.theme=theme;},[language,theme]);
  useEffect(()=>{
    if("__TAURI_INTERNALS__" in window&&preferNativeConnection(sessionStorage))void openLocalWorkspace();
    else if(connection.token)void connect(connection);
    return()=>{connectionGeneration.current++;};
  },[]);
  const nativeAvailable="__TAURI_INTERNALS__" in window&&nativeConnectionAllowed(connection.base,connection.kind);
  const localProfile:WorkspaceProfile={id:workspaceId({base:"http://127.0.0.1:43821",kind:"native"}),name:"",base:"http://127.0.0.1:43821",kind:"native"};
  const availableWorkspaces="__TAURI_INTERNALS__" in window&&!workspaces.some(profile=>profile.kind==="native")?[localProfile,...workspaces]:workspaces;
  const connectionForm=(initial:ConnectionSettings,name="",allowTeam=false)=><ConnectionForm t={t} initial={initial} name={name} named={adding} busy={busy}
    onConnect={(next,name)=>void connect(next,name)} onLocal={"__TAURI_INTERNALS__" in window?name=>void openLocalWorkspace(name):undefined}
    onError={setError} onTeam={allowTeam?setTeamConnection:undefined}/>;
  if(teamConnection!==null)return <main className="connection"><div className="connection-card product-dialog"><Brand/><p>{language==="de"?"Team-Arbeitsbereich":"Team workspace"} · {teamConnection||location.origin}</p><TeamPanel client={new Client("",teamConnection)}/>{"__TAURI_INTERNALS__" in window&&<details><summary>Lokale Werkzeuge einrichten</summary><ToolRuntimeSetup/></details>}<button onClick={()=>setTeamConnection(null)}>{t("disconnect")}</button><Preferences t={t} scope={appearanceScope(client)}/></div></main>;
  if(ready)return <QueryClientProvider client={workspaceQueries}><Workspace key={workspaceVersion} client={client} nativeAvailable={nativeAvailable} serverConnection={!!connection.base&&!nativeAvailable}
    workspaceSwitcher={<WorkspaceSwitcher profiles={availableWorkspaces} selected={workspaceId(connection)} busy={busy} t={t} onSelect={selectWorkspace} onAdd={()=>{setAdding(true);setReconnecting(null);setError(undefined);}} onEdit={profile=>{setEditing(profile);setError(undefined);}}/>}
    connectionError={!adding&&!editing?error:undefined}/>
    {editing&&<Dialog title={t("editWorkspaceConnection")} onClose={closeEditDialog} className="workspace-connection-dialog">
      <ErrorNotice error={error} t={t}/>
      <WorkspaceConnectionEditor profile={editing} active={editing.id===workspaceId(connection)} busy={busy} t={t}
        onSave={changes=>void saveWorkspace(editing,changes)} onRemove={()=>removeWorkspace(editing)} onCancel={closeEditDialog}/>
    </Dialog>}
    {adding&&<Dialog title={t(reconnecting?"connect":"addWorkspace")} onClose={closeConnectionDialog} className="workspace-connection-dialog">
      <p>{t("connectText")}</p><ErrorNotice error={error} t={t}/>
      {connectionForm({base:reconnecting?.base??"",token:"",kind:"server"},reconnecting?.name??"")}
      <div className="actions"><button onClick={closeConnectionDialog}>{t("cancel")}</button></div>
    </Dialog>}
  </QueryClientProvider>;
  return <main className="connection"><div className="connection-card"><Brand/><p className="eyebrow">{t("preview")}</p>
    <h1>{t("connect")}</h1><p>{t("connectText")}</p><ErrorNotice error={error} t={t}/>
    {connectionForm(connection,"",true)}<DesktopUpdates/><Preferences t={t} scope={appearanceScope(client)}/></div></main>;
}
function ConnectionForm({t,initial,name,named,busy,onConnect,onLocal,onError,onTeam}:{t:T,initial:ConnectionSettings,name:string,named:boolean,busy:boolean,onConnect:(next:ConnectionSettings,name:string)=>void,onLocal?:((name:string)=>void),onError:(error:unknown)=>void,onTeam?:((base:string)=>void)}){
  return <form className="workspace-connection-form" onSubmit={e=>{e.preventDefault();const data=new FormData(e.currentTarget);try{onConnect({token:String(data.get("token")??"").trim(),base:connectionEndpoint(String(data.get("endpoint")??"")),kind:"server"},String(data.get("name")??"").trim());}catch{onError(new Error("invalidServerAddress"));}}}>
    {named&&<label>{t("workspaceName")}<input name="name" defaultValue={name} placeholder={t("workspaceNamePlaceholder")} maxLength={80} disabled={busy}/></label>}
    <label>{t("serverAddress")}<input name="endpoint" type="url" autoComplete="url" defaultValue={initial.base} placeholder="https://cowork.example" disabled={busy}/></label>
    <p className="muted">{t("serverAddressHint")}</p>
    <label>{t("token")}<input name="token" type="password" autoComplete="off" required minLength={48} defaultValue={initial.token} disabled={busy}/></label>
    <p className="muted">{t("tokenHelp")}</p><button className="primary" disabled={busy}><Plug size={18}/>{busy?t("connecting"):t("connectAction")}</button>
    {onTeam&&<button type="button" disabled={busy} onClick={e=>{try{onTeam(connectionEndpoint(String(new FormData(e.currentTarget.form!).get("endpoint")??"")));}catch{onError(new Error("invalidServerAddress"));}}}>{t("signInWithTeam")}</button>}
    {onLocal&&<button type="button" disabled={busy} onClick={e=>onLocal(String(new FormData(e.currentTarget.form!).get("name")??"").trim())}>{t("openLocalWorkspace")}</button>}
  </form>;
}
function NavGroup({label,icon:Icon,open,onToggle,children}:{label:string;icon:LucideIcon;open:boolean;onToggle:()=>void;children:ReactNode}){
  const id=useId();
  return <div className={"nav-group "+(open?"open":"")}><button type="button" className="nav-item nav-group-toggle" aria-expanded={open} aria-controls={id} onClick={onToggle}><Icon size={17}/><span>{label}</span><ChevronRight size={15} className="nav-chevron"/></button>
    <div id={id} className="nav-submenu" hidden={!open}>{children}</div></div>;
}
function Brand(){return <div className="brand"><span className="brand-mark"><span/></span><span>LocalAI <strong>Cowork</strong></span></div>;}
function Preferences({t,scope="local"}:{t:T;scope?:string}){
  const {setPreferences:setAppearance}=useAppearancePreferences(scope);
  const {language,setLanguage,theme,setTheme}=useView();
  return <div className="preferences"><label className="sr-only" htmlFor="language">{t("language")}</label>
    <select id="language" value={language} onChange={e=>setLanguage(e.target.value as "de"|"en")}><option value="de">Deutsch</option><option value="en">English</option></select>
    <button className="icon" aria-label={t("toggleTheme")} onClick={()=>{setAppearance({followSystemTheme:false});setTheme(theme==="light"?"dark":"light");}}>{theme==="light"?<Moon size={18}/>:<Sun size={18}/>}</button></div>;
}
function Workspace({client,nativeAvailable,serverConnection,workspaceSwitcher,connectionError}:{client:Client,nativeAvailable:boolean,serverConnection:boolean,workspaceSwitcher:ReactNode,connectionError:unknown}){
  const t=useTranslation();const qc=useQueryClient();
  // Desktop only: installs the document tool runtime once at first start (with backoff).
  useDocumentAutoSetup(nativeAvailable&&!serverConnection);
  const language=useView(v=>v.language);
  const {preferences:appearance,setPreferences:setAppearance}=useAppearancePreferences(appearanceScope(client));
  useEffect(()=>applyAppearancePreferences(document,appearance),[appearance]);
  useEffect(()=>{if(!appearance.followSystemTheme)return;const media=window.matchMedia("(prefers-color-scheme: dark)");const update=()=>useView.getState().setTheme(media.matches?"dark":"light");update();media.addEventListener("change",update);return()=>media.removeEventListener("change",update);},[appearance.followSystemTheme]);
  const [settingsOpen,setSettingsOpen]=useState(false);
  const [settingsVisited,setSettingsVisited]=useState(false);
  const [settingsSection,setSettingsSection]=useState<SettingsSection>("general");
  const [workspaceView,setWorkspaceView]=useState<"chat"|"tasks"|"project"|"knowledge">("chat");
  const [knowledgeVisited,setKnowledgeVisited]=useState(false);
  const [managedChat,setManagedChat]=useState<Order|null>(null);
  const [chatCloseBlocked,setChatCloseBlocked]=useState(false);
  const [commandOpen,setCommandOpen]=useState(false),[commandHelp,setCommandHelp]=useState(false),[commandSearch,setCommandSearch]=useState("");
  const [libraryTab,setLibraryTab]=useState<"workflows"|"personalities"|"agents">("workflows");
  const [settingsTab,setSettingsTab]=useState<SettingsTabRequest|null>(null);
  const [workbenchRequest,setWorkbenchRequest]=useState<WorkbenchRequest|null>(null);
  const [navGroups,setNavGroups]=useState<string[]>(()=>{try{const value=JSON.parse(localStorage.getItem("cowork-v2-nav-groups")??"[]");return Array.isArray(value)?value.filter(item=>typeof item==="string"):[];}catch{return [];}});
  function toggleNavGroup(id:string){setNavGroups(current=>{const next=current.includes(id)?current.filter(item=>item!==id):[...current,id];try{localStorage.setItem("cowork-v2-nav-groups",JSON.stringify(next));}catch{/* Navigation state is only a convenience. */}return next;});}
  const [menuOpen,setMenuOpen]=useState(false);
  const menuButtonRef=useRef<HTMLButtonElement>(null);
  const [sidebarCollapsed,setSidebarCollapsed]=useState(()=>{try{return localStorage.getItem("cowork-v2-sidebar-collapsed")==="1";}catch{return false;}});
  function leaveMenu(){if(menuOpen)menuButtonRef.current?.focus({preventScroll:true});setMenuOpen(false);setNav(false);}
  function openSettings(section:SettingsSection="general",tab?:string){setSettingsSection(section);setSettingsTab(tab?{section,tab,nonce:Date.now()}:null);setSettingsVisited(true);setSettingsOpen(true);leaveMenu();}
  function openLibrary(tab:"workflows"|"personalities"|"agents"){setLibraryTab(tab);setDialog("workflow");leaveMenu();}
  const desktopNavigation=useMedia("(min-width: 1024px)");
  const navigationRef=useRef<HTMLElement>(null);
  const workspaceActive=useRef(true);
  useEffect(()=>{workspaceActive.current=true;return()=>{workspaceActive.current=false;};},[]);
  const {project,order,selectProject,selectOrder,selectLocation}=useView();
  const [dialog,setDialog]=useState<"project"|"order"|"archive"|"provider"|"activity"|"search"|"security"|"workflow"|"money"|"product"|null>(null);
  const [workflowCloseBlocked,setWorkflowCloseBlocked]=useState(false);
  const [focusRun,setFocusRun]=useState("");
  const activity=useQuery({queryKey:["activity-count"],queryFn:()=>client.activity(),refetchInterval:1500});
  const terminalStatus=useQuery({queryKey:["owner-terminal-status",client.base],queryFn:()=>productRequest<{active_sessions:number}>(client,"/terminals/status"),refetchInterval:1500,retry:false});
  const activeRunCount=activity.data?.items.filter(item=>!terminal(item.run)&&item.run.state!=="interrupted").length??0;
  const activeTerminalCount=terminalStatus.data?.active_sessions??0;
  const activeCount=activeRunCount+activeTerminalCount+(terminalStatus.isError?1:0);
  const [confirmNativeClose,setConfirmNativeClose]=useState(false);
  const completionPreferences=useAgentDefaults(client);
  const startViewApplied=useRef(false);
  useEffect(()=>{if(startViewApplied.current||!completionPreferences.data)return;startViewApplied.current=true;if(completionPreferences.data.value.default_start_view==="settings")openSettings();else if(completionPreferences.data.value.default_start_view==="work"){setWorkspaceView("chat");setSettingsOpen(false);};},[completionPreferences.data]);
  const knownRunStates=useRef(new Map<string,RunRecord["state"]>());
  const completionLookups=useRef(new Set<string>());
  useEffect(()=>{
    const current=activity.data?.items.map(item=>item.run)??[],finished=finishedTransitions(knownRunStates.current,current),prefs=completionPreferences.data?.value;
    const announce=(run:RunRecord)=>{if(!prefs)return;void announceCompletion({notifications:prefs.notifications_enabled??true,sound:prefs.sounds_enabled??false,title:language==="de"?"LocalAI Cowork · Lauf beendet":"LocalAI Cowork · Run finished",body:run.state==="completed"?(language==="de"?"Ein Lauf wurde abgeschlossen.":"A run completed."):(language==="de"?"Ein Lauf wurde beendet. Details im Verlauf.":"A run finished. See history for details."),tag:run.id}).catch(setError);};
    for(const run of finished)announce(run);
    const visible=new Set(current.map(run=>run.id));
    for(const [id,state] of knownRunStates.current){if(["completed","failed","canceled","expired"].includes(state)||visible.has(id)||completionLookups.current.has(id))continue;completionLookups.current.add(id);void client.detail(id).then(value=>{for(const run of finishedTransitions(knownRunStates.current,[value.run]))announce(run);}).catch(()=>{}).finally(()=>completionLookups.current.delete(id));}
  },[activity.data,completionPreferences.data,language]);
  useEffect(()=>{
    if(!appearance.confirmOnClose||!activeCount)return;
    const before=(event:BeforeUnloadEvent)=>event.preventDefault();window.addEventListener("beforeunload",before);
    let stop:(()=>void)|undefined,live=true;
    if("__TAURI_INTERNALS__" in window)void import("@tauri-apps/api/window").then(async({getCurrentWindow})=>{const unlisten=await getCurrentWindow().onCloseRequested(event=>{event.preventDefault();setConfirmNativeClose(true);});if(live)stop=unlisten;else unlisten();}).catch(()=>{});
    return()=>{live=false;stop?.();window.removeEventListener("beforeunload",before);};
  },[activeCount,appearance.confirmOnClose]);

  function openOrder(projectId:string,orderId:string,runId=""){if(!workspaceActive.current)return;selectLocation(projectId,orderId);setFocusRun(runId);setDialog(null);setSettingsOpen(false);setWorkspaceView("chat");leaveMenu();}
  const [search,setSearch]=useState("");const [nav,setNav]=useState(false);const [error,setError]=useState<unknown>();const [creating,setCreating]=useState(false);
  // Wide windows keep the sidebar in the layout and remember whether it is collapsed; narrow windows show it as an overlay.
  const sidebarVisible=desktopNavigation?!sidebarCollapsed:nav;
  useEffect(()=>{if(!sidebarVisible)setMenuOpen(false);},[sidebarVisible]);
  function toggleSidebar(){
    if(!desktopNavigation){setNav(!nav);return;}
    setSidebarCollapsed(current=>{const next=!current;try{localStorage.setItem("cowork-v2-sidebar-collapsed",next?"1":"0");}catch{/* Sidebar state is only a convenience. */}return next;});
  }
  useEffect(()=>{
    if(desktopNavigation||!nav)return;
    const previous=document.activeElement as HTMLElement|null;const panel=navigationRef.current;
    const focusable=()=>Array.from(panel?.querySelectorAll<HTMLElement>("button,input,select,a[href]")??[]).filter(e=>e.offsetParent!==null);
    focusable()[0]?.focus();
    const trap=(e:KeyboardEvent)=>{
      if(document.querySelector("dialog[open]"))return;
      if(e.key==="Escape"){e.preventDefault();setNav(false);return;}
      if(e.key!=="Tab")return;
      const list=focusable();const first=list[0],last=list.at(-1);
      if(e.shiftKey&&document.activeElement===first){e.preventDefault();last?.focus();}
      if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first?.focus();}
    };
    document.addEventListener("keydown",trap);return()=>{document.removeEventListener("keydown",trap);previous?.focus();};
  },[nav,desktopNavigation]);
  const projects=useQuery({queryKey:["projects"],queryFn:client.projects});
  const independentSpace=useQuery({queryKey:["independent-chat-space"],queryFn:()=>client.one("/independent-chat-space","Project"),enabled:project===independentChats,retry:false});
  const projectReady=!!project&&(project!==independentChats||independentSpace.isSuccess);
  const orders=useQuery({queryKey:["orders",project],queryFn:()=>client.orders(project),enabled:projectReady});
  const providers=useQuery({queryKey:["providers"],queryFn:client.providers});
  const health=useQuery({queryKey:["health"],queryFn:client.health,refetchInterval:5000});
  const userProjects=visibleProjects(projects.data??[]);
  const selectedProject=userProjects.find(p=>p.id===project);
  const selectedOrder=orders.data?.find(o=>o.id===order);
  useEffect(()=>{if(projects.isSuccess&&project!==independentChats&&!selectedProject)selectProject(independentChats);},[projects.isSuccess,project,selectedProject,selectProject]);
  async function create(e:FormEvent<HTMLFormElement>){
    e.preventDefault();setCreating(true);setError(undefined);
    const title=String(new FormData(e.currentTarget).get("name")??"");
    try {
      if(dialog==="project"){const p=await client.createProject({name:title});await qc.invalidateQueries({queryKey:["projects"]});if(workspaceActive.current)selectProject(p.id);}
      else {const o=await client.createOrder({project_id:project,title});await qc.invalidateQueries({queryKey:["orders",project]});if(workspaceActive.current)selectOrder(o.id);}
      setDialog(null);setNav(false);
    }catch(e){setError(e);}finally{setCreating(false);}
  }
  async function newChat(){
    if(creating)return;setCreating(true);setError(undefined);
    try{
      let projectId=project;
      if(!projectId||projectId===independentChats){projectId=(await client.one("/independent-chat-space","Project")).id;}
      const chat=await client.createOrder({project_id:projectId,title:language==="de"?"Neuer Chat":"New Chat"});
      await qc.invalidateQueries({queryKey:["orders",projectId]});openOrder(projectId,chat.id);
    }catch(reason){setError(reason);}finally{setCreating(false);}
  }
  const commandActions:WorkspaceCommandActions={
    newChat,newProject:()=>setDialog("project"),search:query=>{setCommandSearch(query??"");setDialog("search");},
    settings:openSettings,tasks:()=>{if(!project)throw new Error(language==="de"?"Wähle zuerst ein Projekt.":"Choose a project first.");setSettingsOpen(false);setWorkspaceView("tasks");},
    projectSettings:()=>{if(!selectedProject)return;setSettingsOpen(false);setWorkspaceView("project");},
    library:tab=>{setLibraryTab(tab);setDialog("workflow");},activity:()=>setDialog("activity"),
    toggleTheme:()=>{setAppearance({followSystemTheme:false});useView.getState().setTheme(useView.getState().theme==="light"?"dark":"light");},help:()=>setCommandHelp(true)
  };
  useCommandShortcut(()=>setCommandOpen(true));
  const commands=workspaceCommands({language,hasProject:!!selectedProject,hasWorkspace:projectReady,hasChat:!!order,actions:commandActions});
  return <ChatDeletionProvider client={client} onDeleted={id=>{if(id===order)selectOrder("");setManagedChat(null);}}><div className={"app"+(sidebarVisible?" sidebar-open":"")}>{confirmNativeClose&&<Dialog title={language==="de"?"Fenster schließen?":"Close window?"} onClose={()=>setConfirmNativeClose(false)}><p>{language==="de"?`${activeRunCount} aktive Läufe und ${activeTerminalCount} Terminalsitzungen bleiben nach dem Schließen aktiv.`:`${activeRunCount} active runs and ${activeTerminalCount} terminal sessions will continue after closing.`}{terminalStatus.isError&&<span> {language==="de"?"Der Status weiterer Terminals ist derzeit unbekannt.":"The status of additional terminals is currently unknown."}</span>}</p><button type="button" onClick={()=>setConfirmNativeClose(false)}>{language==="de"?"Weiterarbeiten":"Keep working"}</button><button type="button" onClick={()=>void import("@tauri-apps/api/window").then(({getCurrentWindow})=>getCurrentWindow().destroy()).catch(setError)}>{language==="de"?"Fenster schließen":"Close window"}</button></Dialog>}<CommandPalette open={commandOpen} onClose={()=>setCommandOpen(false)} commands={commands} language={language}/>{commandHelp&&<Dialog title={language==="de"?"Chatbefehle":"Chat commands"} onClose={()=>setCommandHelp(false)}><SlashCommandHelp language={language}/></Dialog>}
    {nav&&<button className="backdrop nav-backdrop" aria-label={t("close")} onClick={()=>setNav(false)}/>}
    <button type="button" className="sidebar-toggle" aria-controls="workspace-sidebar" aria-expanded={sidebarVisible} aria-label={sidebarVisible?(language==="de"?"Seitenleiste einklappen":"Collapse sidebar"):(language==="de"?"Seitenleiste ausklappen":"Expand sidebar")} title={sidebarVisible?(language==="de"?"Seitenleiste einklappen":"Collapse sidebar"):(language==="de"?"Seitenleiste ausklappen":"Expand sidebar")} onClick={toggleSidebar}>{sidebarVisible?<ChevronLeft size={15}/>:<ChevronRight size={15}/>}</button>
    <aside ref={navigationRef} id="workspace-sidebar" className={"sidebar "+(nav?"is-open":"")} aria-label={t("navigation")}
      inert={!sidebarVisible} aria-hidden={!sidebarVisible} role={!desktopNavigation&&nav?"dialog":undefined} aria-modal={!desktopNavigation&&nav?true:undefined}
      onKeyDown={event=>{if(menuOpen&&event.key==="Escape"){event.preventDefault();event.stopPropagation();setMenuOpen(false);menuButtonRef.current?.focus({preventScroll:true});}}}>
      <div className="sidebar-header"><button ref={menuButtonRef} type="button" className="icon app-menu-button" aria-label={menuOpen?(language==="de"?"Menü schließen":"Close menu"):(language==="de"?"Menü öffnen":"Open menu")} aria-expanded={menuOpen} aria-controls="app-menu" onClick={()=>setMenuOpen(current=>!current)}>{menuOpen?<X size={19}/>:<Menu size={19}/>}</button><Brand/></div>
      <button className="new-order primary" disabled={creating} onClick={()=>void newChat()}><Plus size={18}/>{t("newOrder")}</button>
      <nav className="primary-nav" aria-label={language==="de"?"Hauptbereiche":"Main areas"}>
      <button className={"nav-item "+(!settingsOpen&&workspaceView==="chat"&&project===independentChats?"selected":"")} aria-current={!settingsOpen&&workspaceView==="chat"&&project===independentChats?"page":undefined} onClick={()=>{selectProject(independentChats);setSettingsOpen(false);setWorkspaceView("chat");}}><MessageSquare size={17}/>{t("orders")}</button>
      <button className="nav-item" onClick={()=>setDialog("activity")}><Inbox size={17}/>{t("inbox")}<span className="count">{activity.data?.attention??"—"}</span></button>
      </nav>
    <section id="app-menu" className="app-menu" hidden={!menuOpen} aria-label={language==="de"?"Weitere Bereiche":"More areas"}>
      <nav className="primary-nav" aria-label={language==="de"?"Weitere Bereiche":"More areas"}>
      <NavGroup label={language==="de"?"Aufgaben & Abläufe":"Tasks & workflows"} icon={CircleCheck} open={navGroups.includes("automation")} onToggle={()=>toggleNavGroup("automation")}>
        <button className={"nav-subitem "+(!settingsOpen&&workspaceView==="tasks"?"selected":"")} aria-current={!settingsOpen&&workspaceView==="tasks"?"page":undefined} disabled={!project} onClick={()=>{setSettingsOpen(false);setWorkspaceView("tasks");leaveMenu();}}>{language==="de"?"Aufgaben":"Tasks"}</button>
        <button className="nav-subitem" disabled={!project} onClick={()=>openLibrary("workflows")}>Workflows</button>
        <button className={"nav-subitem "+(settingsOpen&&settingsSection==="automation"?"selected":"")} onClick={()=>openSettings("automation")}>{language==="de"?"Skills & Zeitpläne":"Skills & schedules"}</button>
      </NavGroup>
      <NavGroup label={language==="de"?"Wissen & Vorlagen":"Knowledge & templates"} icon={BookOpen} open={navGroups.includes("knowledge")} onToggle={()=>toggleNavGroup("knowledge")}>
        <button className={"nav-subitem "+(!settingsOpen&&workspaceView==="knowledge"?"selected":"")} aria-current={!settingsOpen&&workspaceView==="knowledge"?"page":undefined} onClick={()=>{setSettingsOpen(false);setKnowledgeVisited(true);setWorkspaceView("knowledge");leaveMenu();}}>{language==="de"?"Meine Wissensbasis":"My knowledge base"}</button>
        <button className="nav-subitem" onClick={()=>openLibrary("agents")}>{language==="de"?"Agenten & Persönlichkeiten":"Agents & personalities"}</button>
        <button className={"nav-subitem "+(settingsOpen&&settingsSection==="knowledge"?"selected":"")} onClick={()=>openSettings("knowledge")}>{language==="de"?"Memory-Einstellungen":"Memory settings"}</button>
      </NavGroup>
      <NavGroup label={language==="de"?"Werkzeuge":"Tools"} icon={CodeXml} open={navGroups.includes("tools")} onToggle={()=>toggleNavGroup("tools")}>
        {([["documents",language==="de"?"Dokumente":"Documents"],["developer","Browser & GitHub"],["terminal","Terminal"]] as const).map(([kind,label])=>
          <button key={kind} className="nav-subitem" disabled={!selectedOrder} title={selectedOrder?undefined:(language==="de"?"Öffne zuerst einen Chat.":"Open a chat first.")} onClick={()=>{if(!selectedOrder)return;setWorkbenchRequest({kind,order:selectedOrder.id,nonce:Date.now()});setSettingsOpen(false);setWorkspaceView("chat");leaveMenu();}}>{label}</button>)}
        <button className={"nav-subitem "+(settingsOpen&&settingsSection==="mcp"?"selected":"")} onClick={()=>openSettings("mcp")}>{language==="de"?"MCP & Connectoren":"MCP & connectors"}</button>
        <button className={"nav-subitem "+(settingsOpen&&settingsSection==="sandbox"?"selected":"")} onClick={()=>openSettings("sandbox")}>{language==="de"?"Sandbox & Umgebungen":"Sandbox & environments"}</button>
      </NavGroup>
      <button className="nav-item" aria-keyshortcuts="Control+k Meta+k" onClick={()=>{setCommandOpen(true);leaveMenu();}}><Command size={17}/><span>{language==="de"?"Suche & Befehle":"Search & commands"}</span><kbd aria-hidden="true">Ctrl K</kbd></button>
      </nav>
    </section>
      <div className="section-heading"><span>{t("projects")}</span>{selectedProject&&<button className="icon" aria-label={language==="de"?"Projekt konfigurieren":"Configure project"} onClick={commandActions.projectSettings}><Settings2 size={15}/></button>}<button className="icon" aria-label={t("newProject")} onClick={()=>setDialog("project")}><Plus size={17}/></button></div>
      <nav aria-label={t("projects")}>{userProjects.map(p=><button key={p.id} className={"nav-item "+(project===p.id?"selected":"")} aria-current={project===p.id?"page":undefined} onClick={()=>{selectProject(p.id);setSettingsOpen(false);setWorkspaceView("chat");}}><Folder size={17}/><span>{p.name}</span></button>)}</nav>
      <div className="section-heading orders-label"><span>{selectedProject?(language==="de"?"Chats im Projekt":"Project chats"):t("orders")}</span><button className="icon" disabled={!projectReady} aria-label={language==="de"?"Archivierte Chats":"Archived chats"} onClick={()=>setDialog("archive")}><RotateCcw size={15}/></button></div>
      <label className="search"><Search size={16}/><input aria-label={t("search")} placeholder={t("search")} value={search} onChange={e=>setSearch(e.target.value)}/></label>
      <nav className="order-list">{orders.data?.filter(o=>o.title.toLocaleLowerCase().includes(search.toLocaleLowerCase())).map(o=>
        <div className="chat-nav-row" key={o.id}><button className={"nav-item "+(o.id===order?"selected":"")} aria-current={o.id===order?"page":undefined}
          onClick={()=>{selectOrder(o.id);setSettingsOpen(false);setWorkspaceView("chat");setNav(false);}}><MessageSquare size={16}/><span>{o.title}</span></button><button className="icon" aria-label={(language==="de"?"Chat verwalten: ":"Manage chat: ")+o.title} onClick={()=>setManagedChat(o)}><Settings2 size={14}/></button></div>)}
        {!!project&&!orders.data?.length&&<p className="muted empty-nav">{t("emptyOrders")}</p>}</nav>
      <footer><button className={"nav-item "+(settingsOpen?"selected":"")} onClick={()=>openSettings()}><Settings2 size={18}/>{t("settings")}</button><Preferences t={t} scope={appearanceScope(client)}/></footer>
    </aside>
    <div className="workspace" inert={!desktopNavigation&&nav}>
      <header className="topbar">
        <span className="breadcrumb">{selectedProject?<>{t("projects")}<ChevronRight size={14}/><strong>{selectedProject.name}</strong></>:<strong>{t("orders")}</strong>}</span>
        {workspaceSwitcher}
        <span className={"connection-state "+(health.isError?"offline":"")} role="status"><span className="status-dot"/>{t(health.isError?"disconnected":"synced")}</span>
        <button className="activity-button" onClick={()=>setDialog("activity")} aria-label={`${t("activity")} · ${activity.data?.total??"—"}`}><Activity size={17}/><span>{activity.data?.total??"—"}</span></button>
      </header>
      <ErrorNotice error={connectionError} t={t}/>
      <ErrorNotice error={error} t={t}/>
      {health.isError&&<div className="banner" role="alert">{t("serverUnavailable")} <button className="quiet" onClick={()=>health.refetch()}>{t("reconnect")}</button></div>}
      <ErrorNotice error={projects.error||independentSpace.error||orders.error} t={t}/>
      <ChatDeletionNotice/>
      {settingsVisited&&<div className="settings-host" hidden={!settingsOpen} inert={!settingsOpen}><SettingsWorkspace client={client} project={project} order={order} nativeAvailable={nativeAvailable} providers={providers.data??[]} section={settingsSection} onSectionChange={section=>{setSettingsSection(section);setSettingsTab(null);}} tabRequest={settingsTab} onClose={()=>setSettingsOpen(false)} onLibrary={tab=>{setLibraryTab(tab??"workflows");setDialog("workflow");}} agentsPanel={<AgentDefaultsPanel client={client} providers={providers.data??[]}/>}/></div>}
      <div className="settings-host workspace-management-host" hidden={settingsOpen||workspaceView!=="tasks"} inert={settingsOpen||workspaceView!=="tasks"}>{project&&<TaskWorkspace key={project} client={client} project={project} providers={providers.data??[]} order={order} onOpenChat={id=>openOrder(project,id)} onStarted={run=>openOrder(project,run.spec.order_id,run.id)}/>}</div>
      <div className="settings-host workspace-management-host" hidden={settingsOpen||workspaceView!=="project"} inert={settingsOpen||workspaceView!=="project"}>{selectedProject&&<ProjectSettingsPanel key={project} client={client} project={project} nativeAvailable={nativeAvailable} onDeleted={()=>{selectProject(independentChats);setWorkspaceView("chat");}}/>}</div>
      <div className="settings-host workspace-management-host" hidden={settingsOpen||workspaceView!=="knowledge"} inert={settingsOpen||workspaceView!=="knowledge"}>{knowledgeVisited&&<PersonalKnowledgeBase client={client}/>}</div>
      <div className="chat-host" hidden={settingsOpen||workspaceView!=="chat"} inert={settingsOpen||workspaceView!=="chat"}>
      {selectedOrder?<OrderWorkspace key={selectedOrder.id} initial={selectedOrder} requestedRun={focusRun} openOrder={openOrder} client={client} nativeAvailable={nativeAvailable} serverConnection={serverConnection} providersReady={providers.isSuccess} onSlash={command=>runWorkspaceSlashCommand(command,commandActions)} providers={providers.data??[]} setup={()=>openSettings("ai","models")} workbenchRequest={workbenchRequest}/>:
        <main className="welcome"><div className="welcome-symbol"><MessageSquare size={30}/></div><p className="eyebrow">{t("begin")}</p><h1>{t("hello")}</h1><p>{t("intro")}</p>
          <div className="welcome-action">{selectedProject?<Folder size={24}/>:<MessageSquare size={24}/>}<div><h2>{t("newOrder")}</h2><p>{selectedProject?(serverConnection?(language==="de"?"Dieses Projekt wird auf deinem Server gespeichert.":"This project is stored on your server."):t("emptyProject")):(language==="de"?"Starte eine Unterhaltung. Du kannst den Chat später einem Projekt zuordnen.":"Start a conversation. You can move the chat to a project later.")}</p></div>
            <button className="primary" disabled={creating} onClick={()=>void newChat()}><Plus size={18}/>{t("newOrder")}</button></div>
          <p className="preview-note">{t("previewNote")}</p></main>}
      </div>
    </div>
    {(dialog==="project"||dialog==="order")&&<Dialog title={t(dialog==="project"?"newProject":"newOrder")} onClose={()=>setDialog(null)}>
      <form onSubmit={create}><label>{t(dialog==="project"?"projectName":"orderName")}<input autoFocus name="name" maxLength={dialog==="project"?200:300} required/></label>
        <ErrorNotice error={error} t={t}/><div className="actions"><button type="button" onClick={()=>setDialog(null)}>{t("cancel")}</button><button className="primary" disabled={creating}>{t("create")}</button></div></form></Dialog>}
    {dialog==="workflow"&&<Dialog className="workflow-dialog" title={t("workflowLibrary")} closeBlocked={workflowCloseBlocked} onClose={()=>setDialog(null)}><WorkflowLibrary client={client} project={project} providers={providers.data??[]} t={t} blockClose={setWorkflowCloseBlocked} initialTab={libraryTab}/></Dialog>}
    {dialog==="archive"&&<Dialog title={language==="de"?"Archivierte Chats":"Archived chats"} onClose={()=>setDialog(null)}><ArchivedChatsPanel key={project} client={client} project={project}/></Dialog>}
    {managedChat&&<Dialog title={language==="de"?"Chat verwalten":"Manage chat"} closeBlocked={chatCloseBlocked} onClose={()=>{if(!chatCloseBlocked)setManagedChat(null);}}><ChatManagementDialog client={client} order={managedChat} projects={userProjects} blockClose={setChatCloseBlocked} onClose={()=>{if(!chatCloseBlocked)setManagedChat(null);}} onChanged={value=>{if(value.archived&&value.order.id===order)selectOrder("");else if(value.order.id===order)openOrder(value.order.project_id,value.order.id);}}/></Dialog>}
    {dialog==="security"&&<Dialog title={t("secretStorage")} onClose={()=>setDialog(null)}><NativeSecrets language={language} t={t}/></Dialog>}
    {dialog==="money"&&<Dialog title={t("moneyBudget")} onClose={()=>setDialog(null)}><PersonalMoneyStatus load={client.personalMoney} language={language} t={t}/></Dialog>}
    {dialog==="activity"&&<Dialog className="work-dialog" title={t("activity")} onClose={()=>setDialog(null)}><ActivityPanel client={client} t={t} open={openOrder}/></Dialog>}
    {dialog==="search"&&<Dialog className="work-dialog" title={t("searchHistory")} onClose={()=>setDialog(null)}><SearchPanel client={client} t={t} project={project} open={openOrder} initialQuery={commandSearch}/></Dialog>}
  </div></ChatDeletionProvider>;
}
type WorkbenchKind="documents"|"developer"|"terminal";
type WorkbenchRequest={kind:WorkbenchKind;order:string;nonce:number};
function OrderWorkspace({initial,client,providers,providersReady,onSlash,setup,requestedRun,openOrder,nativeAvailable,serverConnection,workbenchRequest}:{initial:Order,client:Client,providers:ProviderProfile[],providersReady:boolean,onSlash:(command:WorkspaceSlashCommand)=>void|Promise<void>,setup:()=>void,requestedRun:string,openOrder:(project:string,order:string,run?:string)=>void,nativeAvailable:boolean,serverConnection:boolean,workbenchRequest?:WorkbenchRequest|null}){
  const t=useTranslation();const qc=useQueryClient();
  const language=useView(v=>v.language);
  const wide=useMedia("(min-width: 1440px)");const small=useMedia("(max-width: 719px)");
  const [draft,setDraft]=useState(initial.draft);const [saved,setSaved]=useState(initial.draft);
  const [base,setBase]=useState(initial.revision);const [saving,setSaving]=useState(false);const [conflict,setConflict]=useState<Order|null>(null);
  const [error,setError]=useState<unknown>();const [busy,setBusy]=useState(false);
  const [provider,setProvider]=useState(""),[providerRevision,setProviderRevision]=useState<number|null>(null);const [access,setAccess]=useState<AccessMode>("confirm_mutations");
  const [output,setOutput]=useState<OutputContract|undefined>();
  const {preferences:appearance}=useAppearancePreferences(appearanceScope(client));
  const agentDefaults=useAgentDefaults(client),agentLibrary=useAgentLibrary(client);
  const chatSettings=useChatConfiguration(client,initial.id),[configurationReady,setConfigurationReady]=useState(false),[agentApplying,setAgentApplying]=useState(false),agentApplication=useRef(0);
  const [agentConfig,setAgentConfig]=useState<EffectiveAgentConfig>({instructions:"",generation:{}});
  const [personalityId,setPersonalityId]=useState(""),[agentId,setAgentId]=useState("");
  const defaultsApplied=useRef(false);
  const [modelBinding,setModelBinding]=useState<import("@cowork/client").ModelRegistryRef|undefined>();
  const [execution,setExecution]=useState<"single"|"workflow">("single");const [workflowChoice,setWorkflowChoice]=useState("");
  const [environment,setEnvironment]=useState<EnvironmentReference|null>(null);
  const [workflowDialog,setWorkflowDialog]=useState(false);const [workflowCloseBlocked,setWorkflowCloseBlocked]=useState(false);
  const journalMatches=useMemo(()=>(operation:WorkflowOperation)=>operation.request.kind==="start"&&operation.request.request.order_id===initial.id,[initial.id]);
  const workflowJournal=useWorkflowRecovery(client,initial.project_id,journalMatches);
  const workflowFrozen=!!workflowJournal.operation||workflowJournal.uncertainPrepare||!!workflowJournal.query.data?.operations.some(operation=>journalMatches(operation)&&operation.acknowledged_at==null);
  useEffect(()=>{
    const operation=workflowJournal.operation;if(!operation||operation.request.kind!=="start")return;
    const request=operation.request.request;
    setExecution("workflow");setWorkflowChoice(workflowSelection(request.workflow_id,request.version_id));
    setWorkflowInputs({selection:workflowSelection(request.workflow_id,request.version_id),value:structuredClone(request.inputs)});
    setAccess(request.access_mode??"confirm_mutations");setBudgets({...defaultRunBudgets,...request.budgets});setNativeTools(request.native_tools??[]);
    setEnvironment(request.environment??null);
    if(operation.receipt?.kind==="started")setRunId(operation.receipt.run_id);
  },[workflowJournal.operation]);
  const workflowChoices=useQuery({queryKey:["workflow-choices",initial.project_id],queryFn:async()=>{
    const items:import("@cowork/client").WorkflowLibraryEntry[]=[];let offset=0;
    while(true){const page=await client.workflows(initial.project_id,offset);items.push(...page.items);if(page.next_offset==null)return items;
      if(page.next_offset<=offset)throw new Error("workflow_load_failed");offset=page.next_offset;}
  },retry:false,refetchInterval:10000});
  const selectedWorkflow=findWorkflowSelection(workflowChoices.data??[],workflowChoice);
  const chosenWorkflowVersion=useQuery({queryKey:["workflow-version",initial.project_id,selectedWorkflow?.workflow.id,selectedWorkflow?.version.id],queryFn:()=>client.workflowVersion(initial.project_id,selectedWorkflow!.workflow.id,selectedWorkflow!.version.id),enabled:execution==="workflow"&&!!selectedWorkflow,retry:false});
  const [workflowInputs,setWorkflowInputs]=useState<{selection:string;value:unknown}|null>(null);
  const documentSourceChecks=useRef(new Set<string>());
  const [documentSourceBusy,setDocumentSourceBusy]=useState(false);
  const documentSourceChecking=useCallback((id:string,pending:boolean)=>{
    if(pending)documentSourceChecks.current.add(id);else documentSourceChecks.current.delete(id);
    setDocumentSourceBusy(documentSourceChecks.current.size>0);
  },[]);
  const workflowInputType=chosenWorkflowVersion.data?.definition.dataflow?.input;
  const workflowInputValue=workflowInputs?.selection===workflowChoice&&workflowInputs.value!==undefined?workflowInputs.value:workflowInputType?defaultDataValue(workflowInputType):undefined;
  const [budgets,setBudgets]=useState({...defaultRunBudgets});
  const [folders,setFolders]=useState<FolderGrant[]>([]);const [folderDialog,setFolderDialog]=useState(false);
  const [mcpTools,setMcpTools]=useState<import("@cowork/client").McpToolSelection[]>([]);const [mcpDialog,setMcpDialog]=useState(false);
  const [nativeTools,setNativeTools]=useState<string[]>([]);
  async function applyPersonality(value:Personality|null){
    const application=++agentApplication.current;setAgentApplying(true);
    const model=value?.value.model;
    try{
    const actual=model?.kind==="codex"?await materializeCodexModelProfile({client},model):model?.kind==="provider"?providers.find(p=>p.id===model.provider_id&&p.revision===model.provider_revision):undefined;
    if(model?.kind==="provider"&&!actual)throw new Error("provider_unavailable");
    if(model?.kind==="codex")await qc.invalidateQueries({queryKey:["providers"]});
    if(application!==agentApplication.current)return false;
    setPersonalityId(value?.id??"");setAgentId("");
    setAgentConfig(value?effectivePersonality(value):{instructions:"",generation:{}});
    if(actual){setProvider(actual.id);setProviderRevision(actual.revision);setModelBinding(undefined);}
    setError(undefined);return true;
    }catch(reason){if(application===agentApplication.current)setError(reason);return false;}finally{if(application===agentApplication.current)setAgentApplying(false);}
  }
  async function applyAgent(value:AgentTemplate|null){
    const application=++agentApplication.current;
    if(!value){setAgentId("");setPersonalityId("");setAgentConfig({instructions:"",generation:{}});setAgentApplying(false);return true;}
    setAgentApplying(true);
    try{
      const config=value.value.configuration;
      const model=config.model.kind!=="inherit"?config.model:value.value.personality?.value.model;
      const actual=model?.kind==="codex"?await materializeCodexModelProfile({client},model):model?.kind==="provider"?providers.find(p=>p.id===model.provider_id&&p.revision===model.provider_revision):undefined;
      if(model?.kind==="codex")await qc.invalidateQueries({queryKey:["providers"]});
      if(model?.kind==="provider"&&!actual)throw new Error("provider_unavailable");
      const snapshot=effectiveAgent(value);
      if(config.skill_ids.length||config.memory_ids.length){
        const [skills,knowledge]=await Promise.all([
          productRequest<import("./productApi").Skill[]>(client,`/projects/${initial.project_id}/skills`),
          productRequest<{document:import("./productApi").Knowledge}[]>(client,`/projects/${initial.project_id}/knowledge?query=`)]);
        config.skill_ids.forEach(id=>{if(!skills.some(s=>s.id===id&&s.enabled))throw new Error("selected_skill_unavailable");});
        config.memory_ids.forEach(id=>{const item=knowledge.find(s=>s.document.id===id)?.document;if(!item)throw new Error("selected_memory_unavailable");if(item.kind==="memory"&&item.memory){const memory=item.memory;if(!memory.enabled||memory.scope.kind==="chat"&&memory.scope.order_id!==initial.id||memory.scope.kind==="agent"&&memory.scope.agent_id!==value.id)throw new Error("selected_memory_unavailable");}});
      }
      if(config.mcp_tools.length){const current=await client.mcpServers(initial.project_id);for(const tool of config.mcp_tools){if(!current.some(s=>s.id===tool.server_id&&s.revision===tool.server_revision&&s.configuration.enabled&&s.configuration.selected_tools.includes(tool.tool_name)))throw new Error("selected_mcp_tool_unavailable");}}
      if(application!==agentApplication.current)return false;
      setAgentId(value.id);setPersonalityId(value.value.personality?.id??"");setAgentConfig(snapshot);
      if(actual){setProvider(actual.id);setProviderRevision(actual.revision);setModelBinding(undefined);}
      setAccess(config.access_mode);setBudgets({...defaultRunBudgets,...config.budgets});
      if(config.native_tools.length)setEnvironment(null);setNativeTools([...config.native_tools]);setMcpTools(structuredClone(config.mcp_tools));setError(undefined);return true;
    }catch(reason){if(application===agentApplication.current)setError(reason);return false;}finally{if(application===agentApplication.current)setAgentApplying(false);}
  }
  function restoreChatConfiguration(value:import("@cowork/client").ChatRunConfiguration){
    agentApplication.current++;const copy=structuredClone(value),config=copy.agent_config;
    setWorkflowInputs(copy.workflow?{selection:workflowSelection(copy.workflow.workflow_id,copy.workflow.version_id),value:structuredClone(copy.workflow.inputs)}:null);
    setAgentConfig(copiedChatAgent(config));
    setAgentId(config.agent_source?.id??"");setPersonalityId(config.personality_source?.id??"");setProvider(copy.provider_id??"");setProviderRevision(copy.provider_revision??null);setAccess(copy.access_mode);setBudgets({...defaultRunBudgets,...copy.budgets});setFolders(copy.folders??[]);setNativeTools(copy.native_tools??[]);setMcpTools(copy.mcp_tools??[]);setEnvironment(copy.environment??null);setModelBinding(copy.model_binding??undefined);setOutput(copy.output??undefined);setExecution(copy.workflow?"workflow":"single");setWorkflowChoice(copy.workflow?workflowSelection(copy.workflow.workflow_id,copy.workflow.version_id):"");setAgentApplying(false);setConfigurationReady(true);
    if(copy.provider_id&&!exactChatProvider(copy,providers))setError(new Error("saved_chat_provider_revision_unavailable"));else setError(undefined);
  }
  async function reloadChatConfiguration(){try{const result=await chatSettings.query.refetch();if(result.error)throw result.error;if(!result.data)throw new Error("chat_configuration_unavailable");chatSettings.discardDraft();chatSettings.adopt(result.data);if(result.data.value)restoreChatConfiguration(result.data.value);else{setAgentConfig({instructions:"",generation:{}});setAgentId("");setPersonalityId("");setProvider("");setProviderRevision(null);setAccess("confirm_mutations");setBudgets({...defaultRunBudgets});setNativeTools([]);setMcpTools([]);setFolders([]);setEnvironment(null);setOutput(undefined);setModelBinding(undefined);setExecution("single");setWorkflowChoice("");setConfigurationReady(true);}chatSettings.setError(undefined);}catch(reason){setError(reason);}}
  async function refreshChatTemplate(){if(agentApplying||chatSettings.pending)return;setAgentApplying(true);try{const source=agentConfig.agent_source??agentConfig.personality_source;if(!source)return;const isAgent=!!agentConfig.agent_source,result=await(isAgent?agentLibrary.agents.refetch():agentLibrary.personalities.refetch());if(result.error)throw result.error;const item=result.data?.find(item=>item.id===source.id&&item.revision>=source.revision);if(!item)throw new Error("library_source_unavailable");if(isAgent)await applyAgent(item as AgentTemplate);else await applyPersonality(item as Personality);}catch(reason){setError(reason);}finally{setAgentApplying(false);}}
  useEffect(()=>{
    if(defaultsApplied.current||!providersReady||!chatSettings.query.isSuccess||chatSettings.invalid)return;
    const stored=chatSettings.pendingRequest?.value??chatSettings.draft?.value??chatSettings.query.data.value;
    chatSettings.adopt(chatSettings.query.data);
    if(stored){defaultsApplied.current=true;restoreChatConfiguration(stored);if(chatSettings.draft&&!chatSettings.pendingRequest){if(chatConfigurationEqual(chatSettings.draft.value,chatSettings.query.data.value))chatSettings.discardDraft();else if(chatSettings.draft.expected_revision!==chatSettings.query.data.revision)chatSettings.setError(new Error("chat_configuration_draft_conflict"));}return;}
    if(!agentDefaults.data||!agentLibrary.agents.isSuccess||!agentLibrary.personalities.isSuccess)return;
    defaultsApplied.current=true;const prefs=agentDefaults.data.value;
    void(async()=>{setAccess(prefs.default_access_mode??"confirm_mutations");setBudgets({...defaultRunBudgets,...prefs.default_budgets});
      const defaultModel=prefs.default_provider_id??providers.map(p=>prefs.provider_defaults?.[p.kind]).find(Boolean);if(defaultModel){const profile=providers.find(p=>p.id===defaultModel);if(profile){setProvider(profile.id);setProviderRevision(profile.revision);}else{setError(new Error("default_provider_unavailable"));return;}}
      const savedAgent=agentLibrary.agents.data?.find(a=>a.id===prefs.default_agent_id);
      if(savedAgent){if(await applyAgent(savedAgent))setConfigurationReady(true);return;}
      if(prefs.default_agent_id){setError(new Error("default_agent_unavailable"));return;}
      const personality=agentLibrary.personalities.data?.find(p=>p.id===prefs.default_personality_id)??agentLibrary.personalities.data?.find(p=>p.value.is_default);
      if(personality&&!await applyPersonality(personality))return;setConfigurationReady(true);
    })();
  },[providersReady,providers,agentDefaults.data,agentLibrary.agents.isSuccess,agentLibrary.personalities.isSuccess,chatSettings.query.data,chatSettings.query.isSuccess,chatSettings.invalid]);
  const nativeAvailability=useQuery({queryKey:["toolbox",client.base],queryFn:()=>productRequest<{programs:string[],browser_available:boolean,office_available:boolean,github_available:boolean}>(client,"/toolbox"),retry:false});
  const nativeChoices=[...(nativeAvailability.data?.programs.length?["execute_program"]:[]),...(nativeAvailability.data?.browser_available?["browser"]:[]),...(nativeAvailability.data?.office_available?["office"]:[]),...(nativeAvailability.data?.github_available?["github"]:[])];
  const availableFolders=useQuery({queryKey:["folders",initial.project_id],queryFn:()=>client.folders(initial.project_id),refetchInterval:10000});
  const [runId,setRunId]=useState(requestedRun);const [rail,setRail]=useState(false);const [tab,setTab]=useState<"conversation"|"results"|"details">("conversation");
  useEffect(()=>{if(requestedRun){setRunId(requestedRun);setTab("conversation");}},[requestedRun]);
  const history=useQuery({queryKey:["history",initial.id],queryFn:()=>client.history(initial.id)});
  const [branchMessage,setBranchMessage]=useState<Message|null>(null);
  const branchRequest=useRef<BranchOrder|null>(null);
  const regeneration=useRef(new PersistentRequestLatch<RegenerationIntent>("cowork.regeneration.v1:"+JSON.stringify([client.base,initial.id]),decodeRegenerationIntent));
  const [pendingRegeneration,setPendingRegeneration]=useState(regeneration.current.pending);
  const [exportDialog,setExportDialog]=useState(false);
  const [exportStatus,setExportStatus]=useState<Key|null>(null);
  const [restoreTarget,setRestoreTarget]=useState<RestoreFile|null>(null);
  const [restoreRequest]=useState(()=>new PersistentRequestLatch<CreateRestoreRun>(workspaceJournalKey(client,JSON.stringify(["restore-intent-v2",initial.id])),value=>{const request=decode("CreateRestoreRun",value);if(request.order_id!==initial.id)throw new Error("execution_plan_original_mismatch");return request;}));
  const [restoreOriginal,setRestoreOriginal]=useState<CreateRestoreRun|null>(()=>restoreRequest.pending&&!restoreRequest.invalid?restoreRequest.prepare(()=>{throw new Error("execution_plan_original_missing");}):null);
  const [importTarget,setImportTarget]=useState<FileUpload|null>(null);
  const [importRequest]=useState(()=>new PersistentRequestLatch<CreateImportRun>(workspaceJournalKey(client,JSON.stringify(["import-intent-v2",initial.id])),value=>{const request=decode("CreateImportRun",value);if(request.order_id!==initial.id)throw new Error("execution_plan_original_mismatch");return request;}));
  const [pendingImport,setPendingImport]=useState<CreateImportRun|null>(()=>importRequest.pending&&!importRequest.invalid?importRequest.prepare(()=>{throw new Error("execution_plan_original_missing");}):null);
  const importApprovalFocus=useRef<string|null>(null);
  const [selectedArtifact,setSelectedArtifact]=useState<Artifact|null>(null);
  const [workbench,setWorkbench]=useState<"documents"|"developer"|"terminal"|null>(null),[visitedWorkbenches,setVisitedWorkbenches]=useState<string[]>([]);
  const backToChatButton=useRef<HTMLButtonElement>(null);
  useEffect(()=>{if(workbench!==null)backToChatButton.current?.focus();},[workbench]);
  function openWorkbench(value:WorkbenchKind){composerMenu.current?.hidePopover();setWorkbench(value);setVisitedWorkbenches(current=>current.includes(value)?current:[...current,value]);}
  function closeWorkbench(){setWorkbench(null);setTab("conversation");requestAnimationFrame(()=>composerMenuButton.current?.focus());}
  // Sidebar shortcuts address one chat; a request is applied once and never follows a later chat switch.
  const handledWorkbenchRequest=useRef(0);
  useEffect(()=>{if(workbenchRequest&&workbenchRequest.order===initial.id&&workbenchRequest.nonce!==handledWorkbenchRequest.current){handledWorkbenchRequest.current=workbenchRequest.nonce;openWorkbench(workbenchRequest.kind);}},[workbenchRequest,initial.id]);
  const [composerPanel,setComposerPanel]=useState<"agent"|"environment"|"context"|"limits"|"output"|"tools"|null>(null);
  const [composerMenuOpen,setComposerMenuOpen]=useState(false),[composerHint,setComposerHint]=useState("");
  const composerMenuId=useId(),composerMenu=useRef<HTMLDivElement>(null),composerMenuButton=useRef<HTMLButtonElement>(null),promptRef=useRef<HTMLTextAreaElement>(null);
  // The menu lives in the top layer, so it is anchored to the plus button by hand.
  function placeComposerMenu(){const button=composerMenuButton.current,menu=composerMenu.current;if(!button||!menu)return;const rect=button.getBoundingClientRect(),width=Math.min(400,innerWidth-16);menu.style.width=width+"px";menu.style.left=Math.max(8,Math.min(rect.left,innerWidth-width-8))+"px";menu.style.bottom=Math.max(8,innerHeight-rect.top+8)+"px";menu.style.maxHeight=Math.max(240,rect.top-16)+"px";}
  useEffect(()=>{if(!composerMenuOpen)return;const close=()=>{try{composerMenu.current?.hidePopover();}catch{/* already closed */}};addEventListener("resize",close);return()=>removeEventListener("resize",close);},[composerMenuOpen]);
  const draftRef=useRef(draft);draftRef.current=draft; const revision=useRef(base);const savePromise=useRef<Promise<Order>|null>(null);
  const savedRef=useRef(saved);const serverOrder=useRef(initial);const conflictRef=useRef(conflict);conflictRef.current=conflict;
  const [singleStartJournal]=useState(()=>new PersistentRequestLatch<Parameters<Client["createRun"]>[0]>(singleStartJournalKey(client,initial.project_id,initial.id),value=>decode("RunRequest",value)));
  const [reviewRequest,setReviewRequest]=useState<Parameters<Client["createRun"]>[0]|null>(()=>singleStartJournal.pending&&!singleStartJournal.invalid?singleStartJournal.prepare(()=>{throw new Error("execution_plan_original_missing");}):null);
  const pendingRequest=useRef<Parameters<Client["createRun"]>[0]|null>(reviewRequest);
  const composerConfiguration=useMemo<import("@cowork/client").ChatRunConfiguration>(()=>({agent_config:structuredClone(agentConfig),provider_id:provider||null,provider_revision:provider?providerRevision:null,access_mode:access,budgets:structuredClone(budgets),folders:structuredClone(folders),native_tools:[...nativeTools],mcp_tools:structuredClone(mcpTools),environment:environment?structuredClone(environment):null,model_binding:modelBinding??null,output:output??null,workflow:execution==="workflow"?chatWorkflowSelection(workflowChoice,workflowInputType?workflowInputValue:undefined):null}),[agentConfig,provider,providerRevision,access,budgets,folders,nativeTools,mcpTools,environment,modelBinding,output,execution,workflowChoice,workflowInputType,workflowInputValue]);
  const configurationChanged=!chatConfigurationEqual(chatSettings.baseline?.value,composerConfiguration),configurationLocked=!configurationReady||agentApplying||chatSettings.busy||chatSettings.pending;
  const configurationUnsettled=configurationLocked||configurationChanged||!!chatSettings.error||chatSettings.query.isError||chatSettings.query.data?.revision!==chatSettings.baseline?.revision;
  useEffect(()=>{const latest=chatSettings.query.data,baseline=chatSettings.baseline;if(!configurationReady||!latest||!baseline||latest.revision===baseline.revision||chatSettings.pending||chatSettings.busy||agentApplying)return;if(configurationChanged||chatSettings.draft||latest.revision<baseline.revision){chatSettings.setError(new Error("chat_configuration_draft_conflict"));return;}chatSettings.adopt(latest);if(latest.value)restoreChatConfiguration(latest.value);else void reloadChatConfiguration();},[chatSettings.query.data,chatSettings.baseline,configurationReady,configurationChanged,chatSettings.pending,chatSettings.busy,agentApplying]);
  useEffect(()=>{if(!configurationReady||agentApplying||chatSettings.pending||chatSettings.busy||chatSettings.error||!chatSettings.query.isSuccess||!configurationChanged||workflowFrozen||busy)return;const timer=setTimeout(()=>void chatSettings.save(composerConfiguration).catch(()=>{}),400);return()=>clearTimeout(timer);},[configurationReady,agentApplying,chatSettings.pending,chatSettings.busy,chatSettings.error,chatSettings.query.isSuccess,configurationChanged,composerConfiguration,chatSettings.save,workflowFrozen,busy]);
  const configurationToPreserve=useRef({ready:false,dirty:false,value:composerConfiguration});configurationToPreserve.current={ready:configurationReady&&!agentApplying&&!chatSettings.pending&&!chatSettings.invalid,dirty:configurationChanged,value:composerConfiguration};
  useEffect(()=>{const preserve=()=>{const value=configurationToPreserve.current;if(value.ready&&value.dirty)chatSettings.stash(value.value);};const beforeUnload=(event:BeforeUnloadEvent)=>{try{preserve();}catch(reason){chatSettings.setError(reason);event.preventDefault();}};window.addEventListener("beforeunload",beforeUnload);return()=>{window.removeEventListener("beforeunload",beforeUnload);try{preserve();}catch(reason){chatSettings.setError(reason);}};},[chatSettings.stash]);
  const runs=useQuery({queryKey:["runs",initial.id],queryFn:()=>client.runs(initial.id),refetchInterval:1000});
  const messages=useQuery({queryKey:["messages",initial.id],queryFn:()=>client.messages(initial.id),refetchInterval:1000});
  const artifacts=useQuery({queryKey:["artifacts",initial.id],queryFn:()=>client.artifacts(initial.id),refetchInterval:1000});
  const selectedRun=runs.data?.find(r=>r.id===runId)??runs.data?.slice().sort((a,b)=>b.created_at-a.created_at)[0];
  const detail=useQuery({queryKey:["run",selectedRun?.id],queryFn:()=>client.detail(selectedRun!.id),enabled:!!selectedRun,refetchInterval:700});
  const workflowView=useQuery({queryKey:["workflow-run",selectedRun?.id],queryFn:()=>client.workflowRun(selectedRun!.id),enabled:!!selectedRun?.spec.workflow,retry:false,refetchInterval:700});
  const run=workflowView.data?.run??detail.data?.run??selectedRun;
  const events=useQuery({queryKey:["events",run?.id],queryFn:async()=>{
    const existing=qc.getQueryData<import("@cowork/client").RunEvent[]>(["events",run!.id])??[];
    const result=[...existing];let batch;
    do{batch=await client.events(run!.id,result.at(-1)?.sequence??0);result.push(...batch);}while(batch.length===500);
    return result;
  },enabled:!!run,refetchInterval:run&&!terminal(run)?700:false});
  const stream=useMemo(()=>{if(run?.spec.workflow)return "";let text="";for(const e of events.data??[]){
    const p=e.payload as Record<string,unknown>;if(e.kind==="model_started")text="";
    if(e.kind==="model_delta")text+=String(p.text??"");if(e.kind==="model_completed")text=String(p.content??"");
  }return text;},[events.data,run?.spec.workflow]);
  async function saveDraft():Promise<Order>{
    while(savePromise.current)await savePromise.current;
    const value=draftRef.current;setSaving(true);
    const p=saveCurrentDraft(client,serverOrder.current,value);savePromise.current=p;
    try{const result=await p;revision.current=result.revision;serverOrder.current=result;savedRef.current=value;setBase(result.revision);setSaved(value);setConflict(null);
      qc.setQueryData<Order[]>(["orders",initial.project_id],old=>old?.map(o=>o.id===result.id?result:o));return result;}
    catch(e){setError(e);if(e instanceof RequestError&&e.status===409)setConflict(await client.order(initial.id));throw e;}
    finally{if(savePromise.current===p)savePromise.current=null;setSaving(false);}
  }
  useEffect(()=>{
    if(draft===saved||conflict||busy)return;
    const timer=setTimeout(()=>void saveDraft().catch(()=>{}),700);return()=>clearTimeout(timer);
    // revision is deliberately managed by the serialized save queue.
  },[draft,saved,conflict,busy]);
  useEffect(()=>{
    const warn=(e:BeforeUnloadEvent)=>{if(draftRef.current!==savedRef.current){e.preventDefault();}};
    window.addEventListener("beforeunload",warn);
    return()=>{window.removeEventListener("beforeunload",warn);
      if(draftRef.current!==savedRef.current&&!conflictRef.current)void saveDraft().catch(()=>{});};
  },[]);
  async function acceptWorkflowStart(operation:WorkflowOperation){
    if(operation.receipt?.kind!=="started")throw new Error("workflowJournalUnavailable");
    setRunId(operation.receipt.run_id);
    const order=await client.order(initial.id);
    if(draftRef.current===savedRef.current){draftRef.current=order.draft;setDraft(order.draft);}
    savedRef.current=order.draft;setSaved(order.draft);serverOrder.current=order;revision.current=order.revision;setBase(order.revision);
    qc.setQueryData<Order[]>(["orders",initial.project_id],old=>old?.map(item=>item.id===order.id?order:item));
    await Promise.all([qc.invalidateQueries({queryKey:["runs",initial.id]}),qc.invalidateQueries({queryKey:["messages",initial.id]})]);
  }
  async function send(e:FormEvent){
    e.preventDefault();
    if(documentSourceChecks.current.size)return;
    const command=!pendingRequest.current&&!workflowFrozen?parseWorkspaceSlashCommand(draftRef.current):null;
    if(!command&&configurationUnsettled&&!pendingRequest.current){setError(new Error("chat_configuration_not_saved"));return;}
    if(command){
      if(command.kind==="invalid"){setError(new Error(command.usage));return;}
      setBusy(true);setError(undefined);const previous=draftRef.current;
      try{draftRef.current="";setDraft("");await saveDraft();await onSlash(command);}
      catch(reason){draftRef.current=previous;setDraft(previous);setError(reason);}finally{setBusy(false);}return;
    }
    if(workflowJournal.busy)return;if(!pendingRequest.current&&(!workflowJournal.query.isSuccess||workflowJournal.query.isFetching)){setError(new Error("workflowJournalUnavailable"));return;}setBusy(true);setError(undefined);
    try {
      if(workflowFrozen){await workflowJournal.begin(()=>{throw new Error("workflowJournalUnavailable");});return;}
      let request=pendingRequest.current;
      if(!request){
        const order=await saveDraft();
        if(execution==="workflow"){
          if(!workflowJournal.ready)throw new Error("workflowJournalUnavailable");
          if(!selectedWorkflow||selectedWorkflow.workflow.revoked)throw new Error(t("workflowUnavailable"));
          if(!chosenWorkflowVersion.data||chosenWorkflowVersion.data.reference.id!==selectedWorkflow.version.id)throw new Error(t("workflowUnavailable"));
          request={kind:"workflow",agent_config:structuredClone(agentConfig),order_id:initial.id,base_revision:order.revision,prompt:draftRef.current,
            inputs:workflowInputForStart(chosenWorkflowVersion.data.definition,workflowInputValue),
            workflow_id:selectedWorkflow.workflow.id,version_id:selectedWorkflow.version.id,access_mode:access,idempotency_key:crypto.randomUUID(),budgets:structuredClone(budgets),native_tools:[...nativeTools],folders:structuredClone(folders),mcp_tools:structuredClone(mcpTools),environment:environment?structuredClone(environment):undefined};
        }else{
          const p=providers.find(item=>item.id===provider&&item.revision===providerRevision);if(!p)throw new Error(t("noProvider"));
          request={order_id:initial.id,base_revision:order.revision,prompt:draftRef.current,provider_id:p.id,
            provider_revision:p.revision,access_mode:access,idempotency_key:crypto.randomUUID(),folders,budgets,mcp_tools:mcpTools,native_tools:[...nativeTools],environment:environment??undefined,output,model_binding:modelBinding,agent_config:structuredClone(agentConfig)};
        }
        request=singleStartJournal.prepare(()=>request!);pendingRequest.current=structuredClone(request);
      }
      setReviewRequest(request);
    }catch(reason){setError(reason);}finally{setBusy(false);}
  }
  async function acceptReviewedStart(created:RunRecord){
    singleStartJournal.confirm();pendingRequest.current=null;setReviewRequest(null);setRunId(created.id);
    setDraft("");setSaved("");draftRef.current="";savedRef.current="";
    const order=await client.order(initial.id);serverOrder.current=order;revision.current=order.revision;setBase(order.revision);
    qc.setQueryData<Order[]>(["orders",initial.project_id],old=>old?.map(item=>item.id===order.id?order:item));
    await Promise.all([qc.invalidateQueries({queryKey:["runs",initial.id]}),qc.invalidateQueries({queryKey:["messages",initial.id]})]);
  }
  function retireReviewedStart(){singleStartJournal.confirm();pendingRequest.current=null;setReviewRequest(null);}
  async function action(fn:()=>Promise<unknown>){setError(undefined);setBusy(true);try{await fn();await Promise.all([qc.invalidateQueries({queryKey:["run",run?.id]}),qc.invalidateQueries({queryKey:["workflow-run",run?.id]})]);await runs.refetch();}catch(e){setError(e);}finally{setBusy(false);}}
  async function prepareContextRequest():Promise<CreateRun>{if(configurationUnsettled)throw new Error("chat_configuration_not_saved");const order=await saveDraft();const p=providers.find(item=>item.id===provider&&item.revision===providerRevision);if(!p)throw new Error(t("noProvider"));return {order_id:initial.id,base_revision:order.revision,prompt:draftRef.current,provider_id:p.id,provider_revision:p.revision,access_mode:access,idempotency_key:crypto.randomUUID(),folders,budgets,mcp_tools:mcpTools,native_tools:[...nativeTools],environment:environment??undefined,output,model_binding:modelBinding,agent_config:structuredClone(agentConfig)};}
  async function branch(e:FormEvent<HTMLFormElement>){
    e.preventDefault();if(!branchMessage)return;setBusy(true);setError(undefined);
    const request=branchRequest.current??{title:String(new FormData(e.currentTarget).get("title")),through_message_id:branchMessage.id,idempotency_key:crypto.randomUUID()};
    branchRequest.current=request;
    try{await saveDraft();const created=await client.branchOrder(initial.id,request);await qc.invalidateQueries({queryKey:["orders",initial.project_id]});
      branchRequest.current=null;setBranchMessage(null);openOrder(created.project_id,created.id);
    }catch(e){setError(e);if(e instanceof RequestError&&definitiveRequestRejection(e.status))branchRequest.current=null;}finally{setBusy(false);}
  }
  function knownAnswerCanRegenerate(message:Message){const source=runs.data?.find(item=>item.id===message.run_id);return !source||answerCanRegenerate(source);}
  async function regenerate(message?:Message){
    if(busy||workflowFrozen||(!regeneration.current.pending&&configurationUnsettled))return;setBusy(true);setError(undefined);
    try{
      let pending=regeneration.current.pending?regeneration.current.prepare(()=>{throw new Error("request_journal_invalid");}):null;
      if(!pending){
        if(!message)throw new Error("regeneration_prompt_missing");
        const source=await client.detail(message.run_id);
        if(!answerCanRegenerate(source.run))throw new Error("regeneration_requires_model_answer");
        const previous=transcript.slice(0,transcript.indexOf(message)).reverse().find(item=>item.role==="user");
        if(!previous)throw new Error("regeneration_prompt_missing");
        if(source.run.spec.project_id!==initial.project_id)throw new Error("workspace_receipt_mismatch");
        let intent:Pick<RegenerationIntent,"template"|"workflow_source">;
        if(source.run.spec.workflow){
          const order=await saveDraft();
          const inputSnapshot=source.run.spec.workflow.inputs!=null?await client.workflowInputSnapshot(source.run.id):undefined;
          intent=workflowRegenerationIntent(source.run,{order_id:initial.id,base_revision:order.revision,idempotency_key:crypto.randomUUID(),
            access_mode:access,budgets,agent_config:structuredClone(agentConfig),folders,native_tools:nativeTools,mcp_tools:mcpTools,environment},previous.content,inputSnapshot);
        }else{
          const template=await prepareContextRequest();template.prompt=previous.content;template.folders=template.folders?.filter(folder=>!folder.order_id);intent={template};
        }
        pending=regeneration.current.prepare(()=>({branch:{title:initial.title.slice(0,250)+" · "+(language==="de"?"Neue Antwort":"New answer"),through_message_id:message.id,idempotency_key:crypto.randomUUID()},...intent}));
      }
      const chat=await prepareRegenerationReview(client,initial,pending);
      regeneration.current.confirm();setPendingRegeneration(false);await qc.invalidateQueries({queryKey:["orders",chat.project_id]});openOrder(chat.project_id,chat.id);
    }catch(reason){if(reason instanceof RequestError&&definitiveRequestRejection(reason.status))regeneration.current.confirm();setError(reason);}finally{setPendingRegeneration(regeneration.current.pending);setBusy(false);}
  }
  async function restoreFiles(e:FormEvent<HTMLFormElement>){
    e.preventDefault();if(!restoreTarget)return;const path=String(new FormData(e.currentTarget).get("path"));setBusy(true);setError(undefined);
    try{
      const order=await saveDraft();
      const request=restoreRequest.prepare(()=>({order_id:initial.id,base_revision:order.revision,idempotency_key:crypto.randomUUID(),
        prompt:t("restore_file")+": "+path,access_mode:"confirm_mutations",restore:{...restoreTarget,path}}));
      setRestoreOriginal(request);
    }catch(e){setError(e);}finally{setBusy(false);}
  }
  async function acceptRestoredStart(created:RunRecord){
      const order=await client.order(initial.id);revision.current=order.revision;serverOrder.current=order;setBase(order.revision);
      qc.setQueryData<Order[]>(["orders",initial.project_id],old=>old?.map(o=>o.id===order.id?order:o));
      await Promise.all([qc.invalidateQueries({queryKey:["runs",initial.id]}),qc.invalidateQueries({queryKey:["messages",initial.id]})]);
      restoreRequest.confirm();setRestoreOriginal(null);setRestoreTarget(null);setRunId(created.id);setTab("conversation");setError(undefined);
  }
  function retireRestoredStart(){restoreRequest.confirm();setRestoreOriginal(null);setRestoreTarget(null);setError(undefined);}
  function prepareRestore(target:RestoreFile){setError(undefined);setRestoreTarget(restoreOriginal?.restore??target);}
  function prepareImport(upload:FileUpload){setError(undefined);setImportTarget(upload);}
  async function importFiles(input:ImportFile){
    setBusy(true);setError(undefined);
    try{
      const order=await saveDraft();
      const request=importRequest.prepare(()=>({order_id:initial.id,base_revision:order.revision,idempotency_key:crypto.randomUUID(),prompt:t("import_file")+": "+input.path,access_mode:access,import:input}));
      setPendingImport(request);
    }catch(reason){setError(reason);}finally{setBusy(false);}
  }
  async function acceptImportedStart(created:RunRecord){
    const order=await client.order(initial.id);revision.current=order.revision;serverOrder.current=order;setBase(order.revision);
    qc.setQueryData<Order[]>(["orders",initial.project_id],old=>old?.map(o=>o.id===order.id?order:o));
    await Promise.all([qc.invalidateQueries({queryKey:["runs",initial.id]}),qc.invalidateQueries({queryKey:["messages",initial.id]})]);
    importRequest.confirm();setPendingImport(null);setImportTarget(null);importApprovalFocus.current=created.id;setRunId(created.id);setWorkbench(null);setTab("conversation");setRail(false);setError(undefined);
  }
  function retireImportedStart(){importRequest.confirm();setPendingImport(null);setImportTarget(null);setError(undefined);}

  async function exportOrder(format:"json"|"md"|"txt"="json"){
    setBusy(true);setError(undefined);setExportStatus(null);
    try{await saveDraft();const result=await client.exportOrder(initial.id);
      const data=format==="json"?JSON.stringify(result,null,2)+"\n":chatTranscriptText(initial.title,transcript,format);
      const status=await deliverDownload(new Blob([data],{type:format==="json"?"application/json":format==="md"?"text/markdown;charset=utf-8":"text/plain;charset=utf-8"}),initial.title.slice(0,120)+(format==="json"?".cowork.json":"."+format),language);
      setExportStatus(deliveryStatusKey(status));if(status!=="canceled")setExportDialog(false);
    }catch(e){setError(e);}finally{setBusy(false);}
  }
  const inherited=history.data?.inherited;
  const filePanel=detail.data&&((detail.data.file_changes?.length??0)>0||(detail.data.file_operations?.length??0)>0||detail.data.uncertain_file_change||detail.data.uncertain_file_operation)?<FileChanges detail={detail.data} events={events.data??[]} client={client} t={t} busy={busy} action={action} restore={prepareRestore} copyRestore/>:undefined;
  const transcript=[...(inherited?.messages??[]),...(messages.data??[])];
  const active=run&&!terminal(run)&&run.state!=="interrupted";
  const unresolvedMcp=detail.data?.mcp_attempts?.some(a=>a.state.state==="dispatch_committed"||a.state.state==="outcome_uncertain")??false;
  const unresolvedManagedProcess=!!detail.data&&managedProcessRecoveryAvailable(detail.data)&&(detail.data.process_attempts??[]).some(attempt=>attempt.state.state==="launch_committed");
  const composerContext={project:initial.project_id,order:initial.id,run,version:chosenWorkflowVersion.data,selection:execution==="workflow"&&selectedWorkflow?{workflow_id:selectedWorkflow.workflow.id,version_id:selectedWorkflow.version.id,inputs:workflowInputValue,access_mode:access,budgets}:undefined};
  const compactComposer=canCompactWorkflowComposer({operation:workflowJournal.operation,context:composerContext,uncertainPrepare:workflowJournal.uncertainPrepare,
    pendingSingleRequest:!!pendingRequest.current,draft,saved,saving,conflict:!!conflict,
    error:error||workflowJournal.error||workflowJournal.query.isError||workflowChoices.isError||chosenWorkflowVersion.isError||selectedWorkflow?.workflow.revoked||messages.error||runs.error||detail.error||workflowView.error||history.error,
    loading:configurationUnsettled||configurationChanged||!workflowJournal.query.isSuccess||workflowJournal.query.isFetching||workflowChoices.isPending||!!selectedWorkflow&&chosenWorkflowVersion.isPending});
  const localCommand=!workflowFrozen&&!pendingRequest.current?parseWorkspaceSlashCommand(draft):null;
  const sendDisabled=documentSourceBusy||!!reviewRequest||singleStartJournal.invalid||configurationUnsettled||busy||workflowJournal.busy||workflowFrozen||(!pendingRequest.current&&(!workflowJournal.query.isSuccess||workflowJournal.query.isFetching))||(execution==="workflow"&&!workflowJournal.ready)||(!pendingRequest.current&&(history.isError||!!conflict||!draft.trim()||(execution==="single"?!providers.some(p=>p.id===provider&&p.revision===providerRevision):!selectedWorkflow||selectedWorkflow.workflow.revoked||!chosenWorkflowVersion.data||chosenWorkflowVersion.isError)));
  const activeComposerPanel=composerPanel==="output"&&execution!=="single"?"agent":composerPanel;
  const german=language==="de",singleAgent=execution==="single";
  const composerTabs:{id:Exclude<typeof composerPanel,null>;label:string;hint:string;icon:LucideIcon;count?:number}[]=[
    {id:"agent",label:singleAgent?(german?"Agent & Persönlichkeit":"Agent & personality"):"Workflow",hint:singleAgent?(german?"Vorlage, Anweisungen, Modus, Temperatur":"Template, instructions, mode, temperature"):(german?"Vorschau des gewählten Ablaufs":"Preview of the selected workflow"),icon:singleAgent?Bot:GitBranch},
    {id:"environment",label:german?"Umgebung":"Environment",hint:german?"Ausführungszugriff und Prozessumgebung":"Execution access and process environment",icon:Box},
    {id:"context",label:german?"Kontext & Ordner":"Context & folders",hint:german?"Ordnerfreigaben und Chat-Kontext":"Folder access and chat context",icon:Folder,count:folders.length},
    {id:"limits",label:german?"Grenzen":"Limits",hint:german?"Token-, Zeit- und Werkzeuglimits":"Token, time and tool limits",icon:Gauge},
    ...(singleAgent?[{id:"output" as const,label:german?"Strukturierte Ausgabe":"Structured output",hint:german?"JSON-Format für das Endergebnis":"JSON format for the final result",icon:Braces,count:output?1:0}]:[]),
    {id:"tools",label:german?"Externe Werkzeuge":"External tools",hint:german?"MCP-Werkzeuge für diesen Lauf":"MCP tools for this run",icon:Plug,count:mcpTools.length}];
  const workbenchViews:{id:WorkbenchKind;label:string;hint:string;icon:LucideIcon}[]=[
    {id:"documents",label:german?"Dokumente":"Documents",hint:german?"Dateien und Office-Dokumente dieses Chats ansehen und bearbeiten":"View and edit this chat's files and office documents",icon:FileText},
    {id:"developer",label:"Browser & GitHub",hint:german?"Webseiten im Entwickler-Browser testen, Git-Änderungen und Pull Requests verwalten":"Test pages in the developer browser, manage Git changes and pull requests",icon:Globe},
    {id:"terminal",label:"Terminal",hint:german?"Befehle im Arbeitsordner dieses Chats ausführen":"Run commands in this chat's working folder",icon:SquareTerminal}];
  const composerOptionCount=folders.length+mcpTools.length+(output&&execution==="single"?1:0);
  return <section className={"order-workspace "+(rail?"rail-open":"")}>
    <div className="order-main">
      <header className="order-header"><div><p className="eyebrow">{t("orders")}</p><h1>{initial.title}</h1></div>
        <div className="order-actions">{workbench!==null&&<button type="button" ref={backToChatButton} className="quiet" onClick={closeWorkbench}><ChevronLeft size={16}/>{german?"Zurück zum Chat":"Back to chat"}</button>}<button className="icon" aria-label={t("exportOrder")} title={t("exportOrder")} onClick={()=>{setExportStatus(null);setExportDialog(true);}}><Download size={20}/></button>
        <button className={"icon "+(rail?"active":"")} aria-label={t("toggleResults")} title={t("toggleResults")} aria-expanded={rail} onClick={()=>{setRail(!rail);setTab("results");}}><PanelRight size={21}/></button></div></header>
      {exportStatus&&!exportDialog&&<p className="order-status" role="status">{t(exportStatus)}</p>}
      {pendingRegeneration&&<p role="status" className="order-status">{language==="de"?"Die ursprüngliche Antwortregeneration ist noch nicht bestätigt.":"The original answer regeneration is not yet confirmed."} <button type="button" disabled={busy||workflowFrozen||regeneration.current.invalid} onClick={()=>void regenerate()}>{language==="de"?"Ursprüngliche Regeneration abgleichen":"Reconcile original regeneration"}</button> <button type="button" disabled={busy} onClick={()=>{if(window.confirm(language==="de"?"Nur die lokale Referenz entfernen? Ein bereits angelegter Chat oder Lauf bleibt erhalten und erscheint in der Chatliste.":"Remove only the local reference? Any chat or run already created remains available in the chat list.")){try{regeneration.current.discard();setPendingRegeneration(false);setError(undefined);void qc.invalidateQueries({queryKey:["orders"]});setBusy(false);}catch(reason){setError(reason);}}}}>{language==="de"?"Lokale Referenz entfernen":"Discard local reference"}</button>{regeneration.current.invalid&&<span>{language==="de"?"Die lokale Referenz ist beschädigt. Es wird keine neue Anfrage gesendet.":"The local reference is invalid. No new request will be sent."}</span>}</p>}
      {workbench===null&&<nav className="mobile-tabs" aria-label={t("currentRun")}>{(["conversation","results","details"] as const).map(v=><button key={v} aria-current={tab===v?"page":undefined}
        className={tab===v?"selected":""} onClick={()=>setTab(v)}>{t(v)}</button>)}</nav>}
      {visitedWorkbenches.includes("documents")&&<div className="chat-workbench" hidden={workbench!=="documents"} inert={workbench!=="documents"}><OfficeWorkbench client={client} order={initial.id} mode={access} onImport={prepareImport}/></div>}
      {visitedWorkbenches.includes("developer")&&<div className="chat-workbench" hidden={workbench!=="developer"} inert={workbench!=="developer"}><DeveloperWorkspace client={client} order={initial.id}/></div>}
      {visitedWorkbenches.includes("terminal")&&<div className="chat-workbench" hidden={workbench!=="terminal"} inert={workbench!=="terminal"}><TerminalDock key={client.base+":"+initial.id} client={client} order={initial.id} onClose={closeWorkbench}/></div>}
      <div hidden={workbench!==null} inert={workbench!==null} className={"conversation "+(tab!=="conversation"?"mobile-hidden":"")}>

        <ImportedHistory client={client} project={initial.project_id} order={initial.id}/>
        {run&&<div className="run-bar"><span className={"badge "+run.state}><Activity size={14}/>{t(run.state)}</span>
          <select aria-label={t("selectRun")} value={run.id} onChange={e=>setRunId(e.target.value)}>
            {runs.data?.slice().sort((a,b)=>b.created_at-a.created_at).map((r,i)=><option key={r.id} value={r.id}>{t("currentRun")} {runs.data.length-i} · {t(r.state)}</option>)}</select>
          {active&&<button className="quiet" disabled={busy||run.state==="cancel_requested"} onClick={()=>void action(()=>client.cancel(run.id))}><Square size={13}/>{t("stopRun")}</button>}
          {run.state==="interrupted"&&<>{!workflowView.data?.input_resume_available&&<button className="quiet" disabled={busy||detail.isPending||detail.isError||unresolvedMcp||unresolvedManagedProcess||!!detail.data?.uncertain_file_change||!!detail.data?.uncertain_file_operation||!!run.spec.workflow&&!workflowCanResume(workflowView.data)} onClick={()=>void action(()=>client.resume(run.id))}><RotateCcw size={15}/>{t(run.spec.workflow?"workflowResumeCompletion":"resume")}</button>}
            <button className="quiet" disabled={busy||detail.isPending||detail.isError||unresolvedMcp||unresolvedManagedProcess||!!detail.data?.uncertain_file_change||!!detail.data?.uncertain_file_operation} onClick={()=>void action(()=>client.cancel(run.id))}>{t("stopRun")}</button></>}</div>}
        {run&&<p className="run-configuration"><strong>{t("currentRun")}</strong>: {run.spec.workflow?(workflowView.data?.name??t("workflowRun")):run.spec.provider?.model??t("nativeFileAction")} · {t(run.spec.access_mode)} · {t(serverConnection?"serverWorkspace":"local")}</p>}
        <div className="messages" aria-live="polite">
          {inherited&&<div className="history-origin"><GitBranch size={18}/><div><strong>{t("inheritedHistory")}</strong><p>{t("inheritedHint")}</p>
            <button className="quiet" onClick={()=>openOrder(initial.project_id,inherited.source_order_id)}>{t("openOrigin")}</button>
            <details><summary>{t("artifactIntegrity")}</summary><code>{inherited.sha256}</code></details></div></div>}
          {!transcript.length&&<div className="empty-order"><MessageSquare size={26}/><h2>{t("startOrder")}</h2><p>{t("emptyMessages")}</p></div>}
          {transcript.map(m=><article className={"message "+m.role} key={m.id}><div className="message-avatar">{m.role==="user"?<User size={14}/>:<span className="mini-mark"/>}</div>
            <div><div className="message-heading"><strong>{t(m.role==="user"?"you":"assistant")}</strong><time>{new Date(m.created_at).toLocaleTimeString(undefined,{hour:"2-digit",minute:"2-digit"})}</time>{m.order_id!==initial.id&&<small>{t("historical")}</small>}</div><div className="message-content"><ChatMessageBody content={m.content} markdown={m.role==="assistant"}/></div><div className="message-actions"><button type="button" className="quiet" onClick={()=>void navigator.clipboard.writeText(m.content).catch(setError)}>{language==="de"?"Kopieren":"Copy"}</button>{m.role==="user"&&<button type="button" className="quiet" disabled={busy||workflowFrozen||!!pendingRequest.current} onClick={()=>{setDraft(m.content);draftRef.current=m.content;document.getElementById("prompt")?.focus();}}>{language==="de"?"Prompt erneut verwenden":"Reuse prompt"}</button>}{m.role==="assistant"&&knownAnswerCanRegenerate(m)&&<button type="button" className="quiet" disabled={busy||workflowFrozen||!!pendingRequest.current} onClick={()=>void regenerate(m)}>{language==="de"?"Antwort regenerieren":"Regenerate answer"}</button>}</div>
              {m.role==="assistant"&&<button className="quiet branch-action" onClick={()=>{branchRequest.current=null;setError(undefined);setBranchMessage(m);}}><GitBranch size={14}/>{t("branchHere")}</button>}</div></article>)}
          {run&&!terminal(run)&&stream&&<article className="message assistant"><div className="message-avatar"><span className="mini-mark"/></div>
            <div><div className="message-heading"><strong>{t("assistant")}</strong><span className="muted">{t("stream")}</span></div><div className="message-content"><ChatMessageBody content={stream}/></div></div></article>}
          {run?.spec.workflow&&workflowView.data&&<WorkflowRunPanel client={client} view={workflowView.data} t={t} busy={busy} artifacts={artifacts.data??[]} approve={(id,input)=>void action(()=>client.approve(run.id,id,input))} changed={async()=>{await Promise.all([qc.invalidateQueries({queryKey:["workflow-run",run.id]}),qc.invalidateQueries({queryKey:["run",run.id]}),qc.invalidateQueries({queryKey:["runs",initial.id]})]);}}/>}
          {!run?.spec.workflow&&run?.state==="waiting_approval"&&detail.data?.approvals.filter(a=>a.decision==null).map(a=><section key={a.id} className="approval" tabIndex={-1} ref={node=>{if(node&&a.tool.name==="import_file"&&importApprovalFocus.current===run.id){importApprovalFocus.current=null;node.scrollIntoView({block:"nearest"});node.focus({preventScroll:true});}}}><div className="approval-title"><ShieldCheck size={21}/><h2>{t("approvalTitle")}</h2></div>
            <p>{t("approvalDescription")}</p><h3>{t(a.tool.name in de?a.tool.name as Key:"details")}</h3>
            {run.spec.mcp_tools?.filter(b=>b.alias===a.tool.name).map(b=><div key={b.alias}><strong>{String((b.definition as Record<string,unknown>).name)}</strong><p>{b.endpoint}</p><p>{t("mcpScopeHint")}</p></div>)}
            {isDocumentTool(a.tool.name)?<DocumentToolSummary name={a.tool.name} args={a.tool.arguments} folders={run.spec.folders??[]} t={t}/>:(["read_text_file","write_text_file","list_files","stat_file","create_directory","rename_file","remove_file","restore_file","import_file"].includes(a.tool.name))?<><dl className="file-scope"><dt>{t("folderContext")}</dt><dd>{run.spec.folders?.find(f=>f.id===(a.tool.arguments as Record<string,unknown>).folder_id)?.order_id?t("workingFiles"):run.spec.folders?.find(f=>f.id===(a.tool.arguments as Record<string,unknown>).folder_id)?.name??t("forbidden")}</dd><dt>{t("filePath")}</dt><dd>{String((a.tool.arguments as Record<string,unknown>).path??"")}</dd>{a.tool.name==="rename_file"&&<><dt>{t("destinationPath")}</dt><dd>{String((a.tool.arguments as Record<string,unknown>).destination??"")}</dd></>}</dl>{a.tool.name==="remove_file"&&<p>{t("removeFileHint")}</p>}
              {a.tool.name==="import_file"&&<p>{String(((a.tool.arguments as Record<string,unknown>).file as Record<string,unknown>)?.file_name??"")} · {String(((a.tool.arguments as Record<string,unknown>).file as Record<string,unknown>)?.size??"")} B · {t((a.tool.arguments as Record<string,unknown>).expected_sha256?"confirmReplace":"destinationNew")}</p>}
              {a.tool.name==="write_text_file"&&<><h4>{t("proposedVersion")}</h4><pre>{String((a.tool.arguments as Record<string,unknown>).content??"")}</pre></>}
              <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(a.tool.arguments,null,2)}</pre></details></>:<pre>{JSON.stringify(a.tool.arguments,null,2)}</pre>}
            <div className="actions"><button disabled={busy} onClick={()=>void action(()=>client.approve(run.id,a.id,{fingerprint:a.fingerprint,allow:false}))}>{t("deny")}</button>
              <button className="primary" disabled={busy} onClick={()=>void action(()=>client.approve(run.id,a.id,{fingerprint:a.fingerprint,allow:true}))}>{t("approve")}</button></div></section>)}
          {!run?.spec.workflow&&run?.state==="waiting_input"&&detail.data?.input_request&&<section className="approval"><h2>{t("question")}</h2>
            <p>{String((detail.data.input_request.arguments as Record<string,unknown>).question??"")}</p>
            <form onSubmit={e=>{e.preventDefault();const answer=String(new FormData(e.currentTarget).get("answer"));void action(()=>client.submitInput(run.id,{request_id:detail.data!.input_request!.id,answer}));}}>
              <label>{t("answer")}<textarea name="answer" required maxLength={32000}/></label><button className="primary" disabled={busy}>{t("submitAnswer")}</button></form></section>}
          {(run?.state==="failed"||run?.state==="expired"||run?.state==="interrupted")&&<div className="banner"><strong>{t(run.state)}</strong><p>{run.error_code&&t(run.error_code in de?run.error_code as Key:"error")}</p></div>}
          {filePanel}
          {detail.data&&<ManagedProcessRecoveryPanel client={client} order={initial.id} detail={detail.data} busy={busy} fresh={!detail.isFetching&&!detail.isError} changed={async()=>{await detail.refetch();await runs.refetch();}}/>}
          {run&&<ToolPreviews client={client} run={run.id} events={events.data??[]} t={t}/>}
        </div>
        <div className="order-controls" role="region" aria-label={t("orderControls")}>
        <ErrorNotice error={error||(!workflowJournal.operation&&workflowJournal.error)||messages.error||runs.error||detail.error||workflowView.error||history.error} t={t}/>
        {conflict&&<div className="conflict" role="alert"><p>{t("draftConflict")}</p><details><summary>{t("confirmDraft")}</summary><pre>{conflict.draft}</pre></details>
          <button onClick={()=>{setDraft(conflict.draft);setSaved(conflict.draft);savedRef.current=conflict.draft;serverOrder.current=conflict;revision.current=conflict.revision;setBase(conflict.revision);setConflict(null);setError(undefined);}}>{t("useServerDraft")}</button>
          <button onClick={()=>{revision.current=conflict.revision;setBase(conflict.revision);setConflict(null);setError(undefined);void saveDraft().catch(()=>{});}}>{t("keepMyDraft")}</button></div>}
        {run&&!!run.spec.mcp_tools?.length&&<McpAttempts attempts={detail.data?.mcp_attempts??[]} interrupted={run.state==="interrupted"} t={t} client={client} refresh={()=>detail.refetch()}/>}
        {!run?.spec.workflow&&<RunBudgetStatus detail={detail.data} t={t}/>}
        <WorkflowOperationPanel compactAcceptedStart={confirmedWorkflowStart(workflowJournal.operation,composerContext)} operation={workflowJournal.operation} busy={busy||workflowJournal.busy} error={workflowJournal.error} t={t}
          commit={()=>void workflowJournal.begin(()=>{throw new Error("workflowJournalUnavailable");})}
          discard={()=>void workflowJournal.action("discard")}
          acknowledge={()=>void workflowJournal.action("acknowledge",acceptWorkflowStart)}/>
        {workflowJournal.uncertainPrepare&&<p role="status">{t("workflowJournalPreparePending")} <button disabled={busy||workflowJournal.busy} onClick={()=>void workflowJournal.begin(()=>{throw new Error("workflowJournalUnavailable");})}>{t("workflowRetrySame")}</button></p>}
        {!workflowJournal.query.isSuccess&&<p role={workflowJournal.query.isError?"alert":"status"}>{t(workflowJournal.query.isError?"workflowJournalUnavailable":"workflowJournalLoading")}</p>}
        <ComposerDisclosure compact={compactComposer} snapshot={!workflowJournal.operation&&confirmedSnapshotWorkflowStart(composerContext)} t={t}>
        <ErrorNotice error={chatSettings.error||chatSettings.query.error} t={t}/>
        {chatSettings.invalid?<p role="alert">{language==="de"?"Die lokale Speicherreferenz wurde geändert oder ist beschädigt. Neue Läufe sind gesperrt.":"The local save reference changed or is damaged. Starting new runs is blocked."} <button type="button" onClick={()=>{chatSettings.discard();void reloadChatConfiguration();}}>{language==="de"?"Lokale Referenz verwerfen und Serverkopie laden":"Discard local reference and load server copy"}</button></p>:chatSettings.pending?<p role="status">{language==="de"?"Die ursprüngliche Agentkonfiguration wird noch gespeichert. Neue Läufe warten auf die Bestätigung.":"The original agent configuration save is awaiting confirmation. New runs wait for it."} <button type="button" disabled={chatSettings.busy} onClick={()=>{if(chatSettings.pendingRequest)void chatSettings.save(chatSettings.pendingRequest.value).catch(()=>{});}}>{language==="de"?"Ursprüngliche Speicherung erneut klären":"Retry original save"}</button></p>:!configurationReady?<p role="status">{language==="de"?"Chatkonfiguration und Standards werden geladen …":"Loading chat configuration and defaults …"}{defaultsApplied.current&&!agentApplying&&<button type="button" onClick={()=>{setConfigurationReady(true);setError(undefined);}}>{language==="de"?"Ohne Bibliotheksstandard fortfahren":"Continue without the library default"}</button>}{chatSettings.query.isError&&<button type="button" onClick={()=>void reloadChatConfiguration()}>{t("retry")}</button>}</p>:<p role="status" className={chatSettings.error?"muted":"sr-only"}>{language==="de"?(configurationChanged?"Agentkonfiguration wird gespeichert …":"Kopierte Chatkonfiguration gespeichert."):(configurationChanged?"Saving agent configuration …":"Copied chat configuration saved.")}{!!chatSettings.error&&<><button type="button" onClick={()=>void chatSettings.query.refetch().then(result=>{if(result.error)throw result.error;return chatSettings.save(composerConfiguration,true);}).catch(()=>{})}>{language==="de"?"Eigenen Konfigurationsentwurf speichern":"Save my configuration draft"}</button><button type="button" onClick={()=>void reloadChatConfiguration()}>{language==="de"?"Serverkonfiguration laden":"Load server configuration"}</button></>}</p>}
        {reviewRequest&&<ExecutionPlanReview key={reviewRequest.idempotency_key} client={client} request={reviewRequest} onStarted={acceptReviewedStart} onRetired={retireReviewedStart}/>}
        {singleStartJournal.invalid&&<p role="alert">{language==="de"?"Die gespeicherte Startanfrage ist beschädigt. Neue Starts bleiben bis zur Klärung gesperrt.":"The saved start request is invalid. New starts remain blocked until reconciliation."}</p>}
        <form className="composer" onSubmit={send} inert={configurationLocked||singleStartJournal.invalid} aria-busy={chatSettings.busy}>
          {execution==="workflow"&&workflowInputType&&!reviewRequest&&<>
            {chosenWorkflowVersion.data&&<WorkflowDocumentSources client={client} order={initial.id} selection={workflowChoice} definition={chosenWorkflowVersion.data.definition} value={workflowInputValue} language={language} disabled={configurationLocked||busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onChecking={documentSourceChecking} onChange={value=>setWorkflowInputs({selection:workflowChoice,value})}/>}
            <fieldset disabled={documentSourceBusy||busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current}><WorkflowDataValueEditor label={language==="de"?"Workflow-Eingaben":"Workflow inputs"} kind={workflowInputType} value={workflowInputValue} onChange={value=>setWorkflowInputs({selection:workflowChoice,value})}/></fieldset>
          </>}
          {execution==="workflow"&&<>{workflowChoices.isError&&<p role="alert">{t("workflow_load_failed")}</p>}{selectedWorkflow?.workflow.revoked&&<p role="alert">{t("workflowUnavailable")}</p>}{chosenWorkflowVersion.isPending&&selectedWorkflow&&<p role="status">{t("loading")}</p>}{chosenWorkflowVersion.isError&&<p role="alert">{t("workflowUnavailable")}</p>}</>}
          <label className="sr-only" htmlFor="prompt">{t("prompt")}</label><textarea id="prompt" ref={promptRef} value={draft} onChange={e=>{setDraft(e.target.value);}} maxLength={64000}
            onKeyDown={e=>{const shortcut=appearance.sendShortcut==="enter"?!e.ctrlKey&&!e.metaKey:e.ctrlKey||e.metaKey;if(e.key==="Enter"&&!e.shiftKey&&!e.nativeEvent.isComposing&&shortcut){e.preventDefault();const button=e.currentTarget.form?.querySelector<HTMLButtonElement>('button[type="submit"]');if(button&&!button.disabled)e.currentTarget.form?.requestSubmit();}}}
            placeholder={t("promptPlaceholder")} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} rows={3}/>
          {!!pendingRequest.current&&<p role="status">{language==="de"?"Die ursprüngliche Startanfrage bleibt gespeichert. Prüfe und bestätige den Plan oben oder verwirf ihn ausdrücklich.":"The original start request remains saved. Review and confirm the plan above or explicitly retire it."}</p>}
          <div className="composer-bar">
            <button type="button" ref={composerMenuButton} className="icon composer-plus" popoverTarget={composerMenuId} aria-label={language==="de"?"Weitere Optionen":"More options"} title={language==="de"?"Weitere Optionen":"More options"} aria-expanded={composerMenuOpen} onClick={()=>{setComposerPanel(null);placeComposerMenu();}}><Plus size={19}/>{!!composerOptionCount&&<span className="composer-plus-badge">{composerOptionCount}</span>}</button>
            {execution==="workflow"?<label className="composer-pill composer-model" title={t("workflowChooseVersion")}><GitBranch size={14}/><span className="sr-only">{t("workflowChooseVersion")}</span><select aria-label={t("workflowChooseVersion")} value={workflowChoice} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current||workflowChoices.isPending} onChange={event=>setWorkflowChoice(event.target.value)}><option value="">{t("workflowChoose")}</option>
              {!!workflowChoice&&!selectedWorkflow&&<option value={workflowChoice} disabled>{t("workflowUnavailable")}</option>}
              {workflowChoices.data?.flatMap(item=>item.versions.slice().reverse().map(version=><option key={workflowSelection(item.id,version.id)} value={workflowSelection(item.id,version.id)} disabled={item.revoked}>{version.name} · {t("workflowVersion")} {version.revision}{item.revoked?" · "+t("workflowRevoked"):""}</option>))}</select></label>:
            !providers.length&&!pendingRequest.current?<button type="button" className="composer-pill composer-connect" onClick={setup}><Plug size={14}/>{t("providerSetup")}</button>:
            <label className="composer-pill composer-model" title={language==="de"?"Modell":"Model"}><Cpu size={14}/><span className="sr-only">{t("chooseProvider")}</span><select aria-label={t("chooseProvider")} value={provider&&!providers.some(p=>p.id===provider&&p.revision===providerRevision)?"__saved_revision_unavailable__":provider} onChange={e=>{const chosen=providers.find(p=>p.id===e.target.value);setProvider(chosen?.id??"");setProviderRevision(chosen?.revision??null);setModelBinding(undefined);setError(undefined);}} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current}>
            {!!provider&&!providers.some(p=>p.id===provider&&p.revision===providerRevision)&&<option value="__saved_revision_unavailable__" disabled>{language==="de"?"Gespeicherte Modellrevision nicht verfügbar":"Saved model revision unavailable"} · r{providerRevision}</option>}
            {!provider&&<option value="">{t(providers.length?"chooseProvider":"noProvider")}</option>}
            {providers.map(p=><option key={p.id} value={p.id}>{p.name} · {p.model}</option>)}</select></label>}
            <label className="composer-pill composer-access" title={language==="de"?"Freigabe: welche Aktionen der Agent ohne Rückfrage ausführen darf":"Approval: which actions the agent may take without asking"}><ShieldCheck size={14}/><span className="sr-only">{t("access")}</span><select aria-label={t("access")} value={access} onChange={e=>setAccess(e.target.value as AccessMode)} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current}>
              {(["read_only","confirm_mutations","confirm_all","autonomous"] as const).map(v=><option key={v} value={v}>{t(v)}</option>)}</select></label>
            <span className="composer-bar-spacer"/>
            <SpeechInputButton language={language} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onText={text=>setDraft(current=>appendTranscript(current,text))} onHint={setComposerHint} focusTarget={()=>promptRef.current?.focus()}/>
            <button type="submit" className="primary composer-send" aria-label={t(busy?"sending":pendingRequest.current?"workflowRetrySame":"send")} title={t(busy?"sending":pendingRequest.current?"workflowRetrySame":"send")} disabled={documentSourceBusy||(localCommand?busy:sendDisabled)}><ArrowUp size={18}/></button>
            <span role="status" className="sr-only">{t(saving?"saving":draft===saved?"saved":"unsaved")}</span>
          </div>
          {composerHint&&<p className="composer-hint" role="status">{composerHint}<button type="button" className="quiet" aria-label={t("close")} onClick={()=>setComposerHint("")}><X size={14}/></button></p>}
          <div id={composerMenuId} ref={composerMenu} popover="auto" className="composer-menu" role="dialog" aria-label={language==="de"?"Laufoptionen":"Run options"}
            onToggle={event=>setComposerMenuOpen(event.newState==="open")}
            onKeyDown={event=>{const target=event.target as HTMLElement;if(event.key==="Enter"&&target instanceof HTMLInputElement)event.preventDefault();}}>
            <div className="composer-menu-root" hidden={activeComposerPanel!==null}>
              <div className="composer-menu-section"><p className="composer-menu-label">{t("workflowExecution")}</p>
                <div className="composer-menu-segmented" role="group" aria-label={t("workflowExecution")}>
                  {(["single","workflow"] as const).map(value=><button type="button" key={value} aria-pressed={execution===value} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onClick={()=>setExecution(value)}>{value==="single"?<Bot size={15}/>:<GitBranch size={15}/>}{t(value==="single"?"workflowSingleAgent":"workflowRun")}</button>)}</div></div>
              <div className="composer-menu-section"><p className="composer-menu-label">{language==="de"?"Einstellungen für diesen Lauf":"Settings for this run"}</p>
                {composerTabs.map(item=><button type="button" key={item.id} className="composer-menu-item" onClick={()=>setComposerPanel(item.id)}><item.icon size={16}/><span><strong>{item.label}</strong><small>{item.hint}</small></span>{!!item.count&&<span className="count">{item.count}</span>}<ChevronRight size={15}/></button>)}</div>
              <div className="composer-menu-section"><p className="composer-menu-label">{language==="de"?"Dateien & Abläufe":"Files & workflows"}</p>
                <button type="button" className="composer-menu-item" onClick={()=>{composerMenu.current?.hidePopover();setTab("results");setRail(true);}}><Folder size={16}/><span><strong>{t("workingFiles")}</strong><small>{language==="de"?"Ergebnisse und Dateien dieses Chats":"Results and files of this chat"}</small></span></button>
                <button type="button" className="composer-menu-item" disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onClick={()=>{composerMenu.current?.hidePopover();setWorkflowDialog(true);}}><GitBranch size={16}/><span><strong>{t("workflowManage")}</strong><small>{language==="de"?"Mehragentenabläufe anlegen und bearbeiten":"Create and edit multi-agent workflows"}</small></span></button></div>
              <div className="composer-menu-section"><p className="composer-menu-label">{german?"Werkzeuge dieses Chats":"Chat tools"}</p>
                {workbenchViews.map(view=><button type="button" key={view.id} className="composer-menu-item" onClick={()=>openWorkbench(view.id)}><view.icon size={16}/><span><strong>{view.label}</strong><small>{view.hint}</small></span><ChevronRight size={15}/></button>)}</div>
              <p className="composer-menu-footer"><Monitor size={13}/>{language==="de"?"Ausführungsort":"Runs on"}: {t(serverConnection?"serverWorkspace":"local")} · {t("runKeepsGoing")}</p>
            </div>
            {activeComposerPanel!==null&&<div className="composer-menu-header"><button type="button" className="quiet" onClick={()=>setComposerPanel(null)}><ChevronLeft size={16}/>{language==="de"?"Zurück":"Back"}</button><strong>{composerTabs.find(item=>item.id===activeComposerPanel)?.label}</strong></div>}
          <div className="composer-option-panel" hidden={activeComposerPanel!=="agent"}>
          {execution==="workflow"&&<><p className="muted">{t("workflowLocalTools")}</p>{chosenWorkflowVersion.data?<WorkflowVersionPreview version={chosenWorkflowVersion.data} providers={providers} t={t}/>:<p className="muted">{t("workflowChoose")}</p>}</>}
          {execution==="single"&&<div className="chat-agent-settings">
            <AgentLibrarySelector client={client} disabled={busy||workflowFrozen||!!pendingRequest.current} personalityId={personalityId} agentId={agentId} onPersonality={value=>void applyPersonality(value)} onAgent={value=>void applyAgent(value)}/>
            <fieldset disabled={busy||workflowFrozen||!!pendingRequest.current}><label>Modus<select value={agentConfig.execution_mode??"execute"} onChange={e=>setAgentConfig({...agentConfig,execution_mode:e.target.value as "execute"|"plan"|"hybrid"})}><option value="execute">Execute</option><option value="plan">Plan</option><option value="hybrid">Hybrid</option></select></label><label>{language==="de"?"Anweisungen für diesen Agent":"Instructions for this agent"}<textarea value={agentConfig.instructions} maxLength={256000} onChange={e=>setAgentConfig({...agentConfig,instructions:e.target.value})}/></label>
            <div className="workflow-fields"><label>{language==="de"?"Temperatur (leer: Providerstandard)":"Temperature (blank: provider default)"}<input type="number" min={0} max={2} step={0.05} value={agentConfig.generation.temperature??""} onChange={e=>setAgentConfig({...agentConfig,generation:{...agentConfig.generation,temperature:e.target.value===""?undefined:Number(e.target.value)}})}/></label>
            <label>{language==="de"?"Timeout in Sekunden":"Timeout in seconds"}<input type="number" min={1} max={1200} value={agentConfig.generation.timeout_ms===undefined?"":agentConfig.generation.timeout_ms/1000} onChange={e=>setAgentConfig({...agentConfig,generation:{...agentConfig.generation,timeout_ms:e.target.value===""?undefined:Number(e.target.value)*1000}})}/></label></div></fieldset>
            {(agentConfig.agent_source??agentConfig.personality_source)&&<p className="muted">{language==="de"?"Gespeicherte Bibliothekskopie":"Saved library copy"} · r{(agentConfig.agent_source??agentConfig.personality_source)!.revision} <button type="button" className="quiet" disabled={busy||workflowFrozen||agentApplying} onClick={()=>void refreshChatTemplate()}>{language==="de"?"Aus Vorlage aktualisieren":"Update from template"}</button></p>}
          </div>}
          {execution==="single"&&<ModelRegistryPicker client={client} providerId={provider} providerRevision={providerRevision??0} value={modelBinding} onChange={setModelBinding} disabled={busy||!!pendingRequest.current}/>}
          </div>
          <div className="composer-option-panel" hidden={activeComposerPanel!=="environment"}>
          <EnvironmentPicker client={client} project={initial.project_id} order={initial.id} value={environment} onChange={setEnvironment} nativeTools={nativeTools} onNativeToolsChange={setNativeTools} nativeChoices={nativeChoices} localAvailable={nativeAvailable&&!serverConnection} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current}/>
          </div>
          <div className="composer-option-panel" hidden={activeComposerPanel!=="context"}>
          <div className="folder-selection"><button type="button" className="quiet" disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onClick={()=>setFolderDialog(true)}><Folder size={16}/>{t("folderContext")} · {folders.length}</button>
            <span>{folders.length?folders.map(f=>f.name).join(", "):t("noFolderAccess")}</span></div>
          {execution==="single"&&<ContextPanel client={client} order={initial.id} scopeKey={JSON.stringify({draft,provider,providerRevision,modelBinding,budgets,folders,mcpTools,nativeTools,environment,output,access,agentConfig})} prepareRequest={prepareContextRequest} disabled={configurationUnsettled||busy||saving||workflowFrozen||!!pendingRequest.current} changed={()=>void qc.invalidateQueries({queryKey:["context-archives",client.base,initial.id]})}/>}
          </div>
          <div className="composer-option-panel" hidden={activeComposerPanel!=="limits"}>
          <RunBudgets value={budgets} setValue={setBudgets} disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} t={t} contextTokens={execution==="single"?providers.find(p=>p.id===provider)?.context_tokens:undefined}/>
          </div>
          {execution==="single"&&<div className="composer-option-panel" hidden={activeComposerPanel!=="output"}><StructuredOutputEditor value={output} onChange={setOutput} disabled={busy||!!pendingRequest.current}/></div>}
          <div className="composer-option-panel" hidden={activeComposerPanel!=="tools"}>
          <div className="folder-selection"><button type="button" className="quiet" disabled={busy||workflowJournal.busy||workflowFrozen||!!pendingRequest.current} onClick={()=>setMcpDialog(true)}><Plug size={16}/>{t("mcpTools")} · {mcpTools.length}</button><span>{mcpTools.length?mcpTools.map(s=>s.tool_name).join(", "):(language==="de"?"Keine externen Werkzeuge ausgewählt":"No external tools selected")}</span></div>
          </div>
          </div>
        </form></ComposerDisclosure>
        </div>
      </div>
      <div hidden={workbench!==null} className={"mobile-panel "+(tab==="conversation"?"mobile-hidden":"")}>{tab==="results"?<Results artifacts={artifacts.data??[]} selected={selectedArtifact} select={setSelectedArtifact} files={<><WorkingFiles order={initial.id} client={client} t={t} run={run} snapshot={detail.data?.workspace_snapshot} prepareImport={prepareImport} copyImport/>{filePanel}</>} t={t}/>:<RunDiagnostics run={run} events={events.data??[]} t={t}/>}</div>
    </div>
    {rail&&wide&&<aside className="results-rail">
      <header><h2>{t("results")}</h2><button className="icon" onClick={()=>setRail(false)} aria-label={t("close")}><X size={19}/></button></header>
      <Results artifacts={artifacts.data??[]} selected={selectedArtifact} select={setSelectedArtifact} files={<><WorkingFiles order={initial.id} client={client} t={t} run={run} snapshot={detail.data?.workspace_snapshot} prepareImport={prepareImport} copyImport/>{filePanel}</>} t={t}/>
      <RunDiagnostics run={run} events={events.data??[]} t={t}/>
    </aside>}
    {rail&&!wide&&!small&&<Dialog className="results-drawer" title={t("results")} onClose={()=>setRail(false)}>
      <Results artifacts={artifacts.data??[]} selected={selectedArtifact} select={setSelectedArtifact} files={<><WorkingFiles order={initial.id} client={client} t={t} run={run} snapshot={detail.data?.workspace_snapshot} prepareImport={prepareImport} copyImport/>{filePanel}</>} t={t}/>
      <RunDiagnostics run={run} events={events.data??[]} t={t}/>
    </Dialog>}
    {branchMessage&&<Dialog title={t("branchHere")} onClose={()=>{if(!busy)setBranchMessage(null);}}><p>{t("branchHint")}</p>
      <form onSubmit={branch}><label>{t("orderName")}<input name="title" maxLength={300} required defaultValue={`${initial.title.slice(0,240)} · ${t("branch")}`} disabled={busy||!!branchRequest.current}/></label>
        <details><summary>{t("branchBoundary")}</summary><pre>{branchMessage.content}</pre></details><ErrorNotice error={error} t={t}/>
        <div className="actions"><button type="button" disabled={busy} onClick={()=>setBranchMessage(null)}>{t("cancel")}</button><button className="primary" disabled={busy||!!conflict}>{t("createBranch")}</button></div>
      </form></Dialog>}
    {(restoreTarget||restoreOriginal)&&<Dialog title={t("restore_file")} closeBlocked={busy||!!restoreOriginal} onClose={()=>{if(!busy&&!restoreOriginal)setRestoreTarget(null);}}>
      {restoreOriginal?<RestoreExecutionPlanReview key={restoreOriginal.idempotency_key} client={client} request={restoreOriginal} onStarted={acceptRestoredStart} onRetired={retireRestoredStart}/>:<>
        <p>{t("runCopyRestoreHint")}</p>
        <form onSubmit={restoreFiles}><label>{t("destinationPath")}<input name="path" defaultValue={restoreTarget?.path} required maxLength={4096} disabled={busy||restoreRequest.invalid}/></label>
          <p>{t("restoreApprovalHint")}</p><ErrorNotice error={restoreRequest.invalid?new Error("request_journal_invalid"):error} t={t}/>
          <div className="actions"><button type="button" disabled={busy} onClick={()=>setRestoreTarget(null)}>{t("cancel")}</button><button className="primary" disabled={busy||!!conflict||restoreRequest.invalid}>{t("prepareRestore")}</button></div>
        </form></>}
    </Dialog>}
    {(importTarget||pendingImport)&&<Dialog title={t("import_file")} closeBlocked={busy||!!pendingImport} onClose={()=>{if(!busy&&!pendingImport)setImportTarget(null);}}>
      {pendingImport?<ImportExecutionPlanReview key={pendingImport.idempotency_key} client={client} request={pendingImport} onStarted={acceptImportedStart} onRetired={retireImportedStart}/>:<>
        {importTarget&&<ImportFileForm upload={importTarget} client={client} t={t} mode={access} setMode={setAccess} busy={busy||!!conflict||importRequest.invalid} submit={importFiles}/>}
        <ErrorNotice error={importRequest.invalid?new Error("request_journal_invalid"):error} t={t}/><button disabled={busy} onClick={()=>setImportTarget(null)}>{t("close")}</button>
      </>}
    </Dialog>}
    {exportDialog&&<Dialog title={t("exportOrder")} onClose={()=>{if(!busy)setExportDialog(false);}}><p>{t("exportHint")}</p>
      {exportStatus&&<p role="status">{t(exportStatus)}</p>}
      <div className="actions"><button disabled={busy||!!conflict} onClick={()=>void exportOrder("md")}>Markdown</button><button disabled={busy||!!conflict} onClick={()=>void exportOrder("txt")}>Text</button></div><ErrorNotice error={error} t={t}/><div className="actions"><button disabled={busy} onClick={()=>setExportDialog(false)}>{t("cancel")}</button><button className="primary" disabled={busy||!!conflict} onClick={()=>void exportOrder()}><Download size={17}/>{t("downloadExport")}</button></div></Dialog>}
    {workflowDialog&&<Dialog className="workflow-dialog" title={t("workflowLibrary")} closeBlocked={workflowCloseBlocked} onClose={()=>{setWorkflowDialog(false);void workflowChoices.refetch();}}><WorkflowLibrary client={client} project={initial.project_id} providers={providers} t={t} blockClose={setWorkflowCloseBlocked}/></Dialog>}
    {mcpDialog&&<Dialog title={t("mcpTools")} onClose={()=>setMcpDialog(false)}><McpTools client={client} project={initial.project_id} selected={mcpTools} setSelected={setMcpTools} t={t}/><div className="actions"><button onClick={()=>setMcpDialog(false)}>{t("close")}</button></div></Dialog>}
    {folderDialog&&<Dialog title={t("folderContext")} onClose={()=>setFolderDialog(false)}><p>{t("folderHint")}</p>
      {availableFolders.isPending&&<p role="status">{t("loading")}</p>}<ErrorNotice error={availableFolders.error} t={t}/>
      {!availableFolders.isPending&&!availableFolders.data?.length&&<p>{t("noConfiguredFolders")}</p>}
      <div className="folder-options">{availableFolders.data?.map(folder=><label key={folder.id}><input type="checkbox" checked={folders.some(f=>f.id===folder.id&&f.revision===folder.revision)} onChange={e=>setFolders(current=>e.target.checked?[...current.filter(f=>f.id!==folder.id),folder]:current.filter(f=>f.id!==folder.id))}/>
        <span>{folder.name}<small>{t(folder.writable?"folderWritable":"folderReadOnly")}</small></span></label>)}</div>
      {folders.some(f=>!availableFolders.data?.some(a=>a.id===f.id&&a.revision===f.revision))&&<p role="alert">{t("folderGrantChanged")}</p>}
      {nativeAvailable?<NativeFolders project={initial.project_id} language={language} t={t}/>:<p>{t("serverNativeAuthority")}</p>}
      <div className="actions"><button onClick={()=>setFolders([])}>{t("clearFolders")}</button><button className="primary" onClick={()=>setFolderDialog(false)}>{t("close")}</button></div>
    </Dialog>}
  </section>;
}
function Results({artifacts,selected,select,t,files}:{artifacts:Artifact[],selected:Artifact|null,select:(a:Artifact)=>void,t:T,files?:ReactNode}){
  const artifact=selected??artifacts.at(-1);
  return <div className="results-content">{files}{!artifacts.length?(!files&&<div className="empty-results"><FileText size={28}/><p>{t("noResults")}</p></div>):<>
    <div className="artifact-list">{artifacts.map(a=><button className={"artifact "+(artifact?.id===a.id?"selected":"")} onClick={()=>select(a)} key={a.id}><FileText size={20}/><span>{a.name}<small>{new Intl.NumberFormat().format(a.size)} B</small></span><ChevronRight size={15}/></button>)}</div>
    {artifact&&<ArtifactWorkbench artifact={artifact} artifacts={artifacts} onSelect={select}/>}</>}</div>;
}
function RunDiagnostics({run,events,t}:{run:RunRecord|undefined,events:unknown[],t:T}){
  return <details className="diagnostics"><summary>{t("diagnostics")}</summary>{run?<><dl><dt>{t("steps")}</dt><dd>{run.steps}</dd><dt>{t("tokens")}</dt><dd>{run.usage.input_tokens??"—"} / {run.usage.output_tokens??"—"}</dd><dt>{t("cost")}</dt><dd>{run.usage.cost??t("unknown")}</dd></dl>{(run.usage.incomplete||run.model_attempts?.some(a=>a.state==="in_flight"))&&<p>{t("usageIncomplete")}</p>}<pre>{JSON.stringify({id:run.id,revision:run.revision,state:run.state,executor:run.spec.executor,policy_revision:run.spec.policy_revision},null,2)}</pre><FrozenOutputPreview value={run.spec.output}/><h3>{t("events")}</h3><pre>{JSON.stringify(events,null,2)}</pre></>:<p>{t("noRuns")}</p>}</details>;
}
