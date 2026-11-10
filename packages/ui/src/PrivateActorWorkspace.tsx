import {useEffect,useRef,useState,type FormEvent} from "react";
import {decode,type ActorModelRegistryView,type Client,type ModelRegistryRef} from "@cowork/client";
import {androidRequest} from "./androidDeviceApi";
import {privateGithubProjects,githubUuid,type PrivateGithubProject} from "./privateGithubApi";
import {PrivateGithubBindingPanel} from "./PrivateGithubBindingPanel";
import {PrivateActorSchedulesPanel} from "./PrivateActorSchedulesPanel";
import {PrivateActorDeviceInterop} from "./PrivateActorDeviceInterop";
import {PrivateMcpBindingsPanel} from "./PrivateMcpBindingsPanel";
import {McpRunTasksPanel} from "./McpRunTasksPanel";
import {AndroidProjectVersionsPanel} from "./AndroidProjectVersionsPanel";
import {AndroidPrivateSharingPanel} from "./AndroidPrivateSharingPanel";
import {AndroidMobileCachePanel} from "./AndroidMobileCachePanel";
import {AndroidAdministrationControlPanel} from "./AndroidAdministrationControlPanel";
import {AndroidMetadataSyncPanel} from "./AndroidMetadataSyncPanel";
import {PrivateWorkflowRegistryPanel} from "./PrivateWorkflowRegistryPanel";
import {PrivateWorkflowRunPanel} from "./PrivateWorkflowRunPanel";
import {AndroidRunRecoveryPanel} from "./AndroidRunRecoveryPanel";
import {DeviceRunRequests} from "./DeviceRunRequests";
import {DeviceRunResult} from "./DeviceRunResult";
import type {DeviceRun} from "./AndroidDevicesPanel";
import {StructuredOutputEditor,type OutputContract} from "./StructuredOutputEditor";
import {useView} from "./view";
import {ActorContextPanel} from "./ActorContextPanel";
import {actorKnowledgeAccess} from "./knowledgeAccessApi";
import {ExternalMemoryPanel} from "./ExternalMemoryPanel";
import {ActorChatConfigurationLatch,actorChatConfiguration,type ActorChatConfiguration,type ActorChatValue} from "./actorChatConfiguration";
import {ActorAgentConfigurationPanel,validateActorRunConfiguration,type ActorAgentSelection,type ActorRunConfiguration} from "./ActorAgentConfigurationPanel";

type Thread={id:string;project_id:string;title:string;revision:number};
type ReleasedTask={id:string;project_id:string;revision:number;name:string;released:boolean};
type Profile={id:string;owner_user_id:string;team_id:null;name:string;revision:number;has_secret:boolean;model_defaults:{model:string;auth_mode:string;context_tokens?:number}};
type RunPlan={thread_id:string;project_id:string;project_revision:number;project_privacy:"private_local";task:{id:string;revision:number}|null;executor_target:{kind:"server_linux"};required_capabilities:string[];model_profile_id:string;snapshot_id:null;input:{expected_chat_configuration_revision?:number;output?:OutputContract;github_binding_id?:string;model_binding?:ModelRegistryRef;project_file_ids:string[];mcp_metadata_ids?:string[];skill_ids?:string[];memory_ids?:string[];expected_provider_revision?:number;expected_mcp_revisions?:Record<string,number>;run_configuration?:ActorRunConfiguration}};
type Original={format:1;key:string;plan:RunPlan;prompt_sha256:string};
type Pending={original:Original;prompt:string};
type ProfilePlan={format:1;key:string;body:{team_id:null;name:string;provider_kind:"openai_compatible";model_defaults:{base_url:string;model:string;context_tokens:number;auth_mode:"bearer"|"none"}}};
const sha=async(value:string)=>Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value)))).map(v=>v.toString(16).padStart(2,"0")).join("");
function savedOriginal(raw:string):Original{
  const v=JSON.parse(raw) as Original;
  if(v?.format!==1||!githubUuid(v.key)||!v.plan||!githubUuid(v.plan.project_id)||!githubUuid(v.plan.thread_id)||!githubUuid(v.plan.model_profile_id)||!Number.isSafeInteger(v.plan.project_revision)||v.plan.project_revision<1||v.plan.project_privacy!=="private_local"||v.plan.executor_target?.kind!=="server_linux"||!/^[0-9a-f]{64}$/.test(v.prompt_sha256)||Object.keys(v).some(k=>!["format","key","plan","prompt_sha256"].includes(k))||Object.hasOwn(v.plan.input,"prompt"))throw new Error("private_original_invalid");
  const task=v.plan.task;if(task!==null&&(!task||!githubUuid(task.id)||!Number.isSafeInteger(task.revision)||task.revision<1||Object.keys(task).some(k=>!["id","revision"].includes(k))))throw new Error("private_original_invalid");
  const ids=v.plan.input.mcp_metadata_ids;if(ids!==undefined&&(!Array.isArray(ids)||ids.length>16||ids.some(id=>!githubUuid(id))||new Set(ids).size!==ids.length))throw new Error("private_original_invalid");
  if(v.plan.input.run_configuration)validateActorRunConfiguration(v.plan.input.run_configuration);
  const expected=v.plan.input.expected_provider_revision;if(expected!==undefined&&(!Number.isSafeInteger(expected)||expected<1))throw new Error("private_original_invalid");
  if(v.plan.input.expected_mcp_revisions&&Object.entries(v.plan.input.expected_mcp_revisions).some(([id,revision])=>!githubUuid(id)||!Number.isSafeInteger(revision)||revision<1))throw new Error("private_original_invalid");
  return v;
}
export function PrivateActorWorkspace({client,session,actorId}:{client:Client;session:string;actorId:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [chatConfig,setChatConfig]=useState<ActorChatConfiguration>(),[chatConfigReady,setChatConfigReady]=useState(false),[restoredSelection,setRestoredSelection]=useState<ActorAgentSelection>(),[profileRevision,setProfileRevision]=useState<number>();
  const chatLatch=useRef<ActorChatConfigurationLatch>(new ActorChatConfigurationLatch());
  const [tasks,setTasks]=useState<ReleasedTask[]>([]),[selectedTask,setSelectedTask]=useState("");
  const [agentSelection,setAgentSelection]=useState<ActorAgentSelection>(),[profilesReady,setProfilesReady]=useState(false);
  const [projects,setProjects]=useState<PrivateGithubProject[]>([]),[project,setProject]=useState(""),[threads,setThreads]=useState<Thread[]>([]),[thread,setThread]=useState(""),[profiles,setProfiles]=useState<Profile[]>([]),[profile,setProfile]=useState(""),[github,setGithub]=useState(""),[mcpSelected,setMcpSelected]=useState<string[]>([]),[models,setModels]=useState<ActorModelRegistryView[]>([]),[model,setModel]=useState(""),[output,setOutput]=useState<OutputContract>(),[runs,setRuns]=useState<DeviceRun[]>([]),[selectedRun,setSelectedRun]=useState(""),[original,setOriginal]=useState<Original>(),[invalidOriginal,setInvalidOriginal]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState(""),[busy,setBusy]=useState(false);
  const [profileOriginal,setProfileOriginal]=useState<ProfilePlan>(),[invalidProfileOriginal,setInvalidProfileOriginal]=useState(false),profilePending=useRef<{plan:ProfilePlan;token:string}|undefined>(undefined);
  const generation=useRef(0),pending=useRef<Pending|undefined>(undefined),prompt=useRef<HTMLTextAreaElement>(null),context=useRef("");
  const [memoryAvailable,setMemoryAvailable]=useState<boolean>();
  useEffect(()=>{let active=true;setMemoryAvailable(undefined);if(session)void actorKnowledgeAccess(client,session).then(v=>{if(active)setMemoryAvailable(v.private_projects);}).catch(()=>{if(active)setMemoryAvailable(undefined);});return()=>{active=false;};},[client.base,session,actorId]);
  context.current=JSON.stringify([client.base,session,actorId,project,thread]);
  const journalKey="cowork.actor_private_run.v1:"+JSON.stringify([client.base,actorId]);
  const profileJournalKey="cowork.actor_private_provider.v1:"+JSON.stringify([client.base,actorId]);
  const api=<T,>(path:string,method="GET",body?:unknown)=>androidRequest<T>(client.base,session,path,method,body);
  const message=(e:unknown)=>{
    const code=e instanceof Error?e.message:String(e);
    const values:Record<string,[string,string]>={private_original_invalid:["Der gespeicherte Laufauftrag ist ungültig.","The recorded run request is invalid."],private_original_pending:["Rufe zuerst den Originalauftrag mit unveränderten Eingaben ab oder verwerfe seine lokale Referenz ausdrücklich.","Retrieve the original request with unchanged input first, or explicitly discard its local reference."],private_context_changed:["Die Auswahl wurde während der Anfrage geändert. Der Originalauftrag bleibt erhalten.","Selection changed during the request. The original request remains retained."],private_choose_context:["Wähle dein privates Projekt, Gespräch und eigenes Modellprofil.","Select your private project, conversation and own model profile."]};
    return values[code]?.[language==="de"?0:1]??tr("Anfrage fehlgeschlagen: ","Request failed: ")+code;
  };
  async function attempt(work:(epoch:number)=>Promise<void>){if(busy)return;const epoch=generation.current;setBusy(true);setError("");setNotice("");try{await work(epoch);}catch(e){if(epoch===generation.current)setError(message(e));}finally{if(epoch===generation.current)setBusy(false);}}
  async function refreshRuns(epoch=generation.current){const scope=JSON.stringify([client.base,session,actorId,project,thread]);if(context.current!==scope)return;const result=await api<{items:DeviceRun[]}>("/runs?limit=200");if(epoch!==generation.current||context.current!==scope)return;setRuns(result.items.filter(r=>r.spec.project_id===project&&r.spec.thread_id===thread&&r.spec.executor_target.kind==="server_linux"));}
  useEffect(()=>{
    setAgentSelection(undefined);setProfilesReady(false);
    const epoch=++generation.current;pending.current=undefined;profilePending.current=undefined;setProfileOriginal(undefined);setInvalidProfileOriginal(false);setOriginal(undefined);setInvalidOriginal(false);setProjects([]);setProfiles([]);setProfile("");setOutput(undefined);setThreads([]);setProject("");setThread("");setGithub("");setMcpSelected([]);setModel("");setRuns([]);setSelectedRun("");setError("");setNotice("");setBusy(false);
    if(!session||!githubUuid(actorId))return;
    try{const raw=localStorage.getItem(journalKey);if(raw){const value=savedOriginal(raw);setOriginal(value);setProject(value.plan.project_id);setThread(value.plan.thread_id);setProfile(value.plan.model_profile_id);setGithub(value.plan.input.github_binding_id??"");setMcpSelected(value.plan.input.mcp_metadata_ids??[]);setOutput(value.plan.input.output);if(value.plan.input.model_binding)setModel(value.plan.input.model_binding.id+":"+value.plan.input.model_binding.version);}}
    catch(e){setInvalidOriginal(true);setError(message(e));}
    try{const raw=localStorage.getItem(profileJournalKey);if(raw){const v=JSON.parse(raw) as ProfilePlan;if(v?.format!==1||!githubUuid(v.key)||Object.keys(v).some(k=>!["format","key","body"].includes(k))||v.body?.team_id!==null||v.body.provider_kind!=="openai_compatible"||typeof v.body.name!=="string"||!v.body.name||v.body.name.length>200||Object.keys(v.body).some(k=>!["team_id","name","provider_kind","model_defaults"].includes(k))||!v.body.model_defaults||Object.keys(v.body.model_defaults).some(k=>!["base_url","model","context_tokens","auth_mode"].includes(k))||!['bearer','none'].includes(v.body.model_defaults.auth_mode)||typeof v.body.model_defaults.base_url!=="string"||v.body.model_defaults.base_url.length>2048||typeof v.body.model_defaults.model!=="string"||v.body.model_defaults.model.length>200||!Number.isSafeInteger(v.body.model_defaults.context_tokens))throw new Error("private_original_invalid");setProfileOriginal(v);}}catch(e){setInvalidProfileOriginal(true);setError(message(e));}
    void Promise.all([privateGithubProjects(client,session),api<Profile[]>("/provider-profiles")]).then(([p,m])=>{if(m.some(v=>v.owner_user_id!==actorId||v.team_id!==null))throw new Error("private_context_changed");if(epoch===generation.current){setProjects(p);setProfiles(m);setProfilesReady(true);}}).catch(e=>{if(epoch===generation.current)setError(message(e));});
    return()=>{generation.current++;pending.current=undefined;profilePending.current=undefined;if(prompt.current)prompt.current.value="";};
  },[client.base,session,actorId]);
  useEffect(()=>{
    const scope=context.current;let canceled=false;setThreads([]);setModels([]);setRuns([]);setSelectedRun("");if(!project)return;
    void api<Thread[]>(`/projects/${project}/threads`).then(values=>{if(values.some(v=>v.project_id!==project||!githubUuid(v.id)))throw new Error("private_context_changed");if(!canceled&&scope===context.current)setThreads(values);}).catch(e=>{if(!canceled&&scope===context.current)setError(message(e));});
    return()=>{canceled=true;};
  },[client.base,session,actorId,project]);
  useEffect(()=>{
    let canceled=false;setModels([]);if(!project||!profile)return;
    void fetch(client.base+`/api/v2/actor-model-registry/entries?project_id=${project}`,{headers:{Authorization:"Bearer "+session},cache:"no-store",credentials:"omit",redirect:"error"}).then(async response=>{if(!response.ok)throw new Error("private_model_registry_unavailable");const rows=(await response.json() as unknown[]).map(v=>decode("ActorModelRegistryView",v));if(rows.some(v=>v.context.actor_id!==actorId||v.context.project_id!==project))throw new Error("private_context_changed");if(!canceled)setModels(rows.filter(v=>v.context.provider.id===profile&&!v.qualification.revoked));}).catch(e=>{if(!canceled)setError(message(e));});return()=>{canceled=true;};
  },[client.base,session,actorId,project,profile]);
  useEffect(()=>{const epoch=generation.current;setRuns([]);if(!thread||!project)return;void refreshRuns(epoch).catch(e=>{if(epoch===generation.current)setError(message(e));});const timer=setInterval(()=>void refreshRuns(epoch).catch(()=>{}),3000);return()=>clearInterval(timer);},[client.base,session,actorId,project,thread]);
  useEffect(()=>{
    let canceled=false;const scope=context.current;setChatConfig(undefined);setChatConfigReady(false);setRestoredSelection(undefined);setProfileRevision(undefined);setAgentSelection(undefined);
    chatLatch.current=new ActorChatConfigurationLatch("cowork.actor-chat.pending.v1:"+JSON.stringify([client.base,actorId,thread]));
    if(!project||!thread)return;
    if(original){setChatConfigReady(true);return;}
    void actorChatConfiguration(client.base,session,project,thread).then(value=>{if(canceled||scope!==context.current)return;setChatConfig(value);if(value.value){const v=value.value;setRestoredSelection(structuredClone(v));setAgentSelection(structuredClone(v));setProfile(v.model_profile_id);setProfileRevision(v.provider_revision);setMcpSelected([...v.mcp_metadata_ids]);setOutput(v.output??undefined);setModel(v.model_binding?v.model_binding.id+":"+v.model_binding.version:"");}setChatConfigReady(true);}).catch(e=>{if(!canceled&&scope===context.current)setError(message(e));});
    return()=>{canceled=true;};
  },[client.base,session,actorId,project,thread,original?.key]);
  async function saveChatConfiguration(epoch=generation.current):Promise<ActorChatConfiguration>{
    const scope=context.current,provider=profiles.find(p=>p.id===profile);
    if(!chatConfigReady||!chatConfig||!agentSelection||!provider)throw new Error("private_agent_settings_unavailable");
    if(profileRevision!==undefined&&profileRevision!==provider.revision)throw new Error("private_chat_provider_changed_reselect_profile");
    const qualification=model?models.find(v=>v.qualification.binding.id+":"+v.qualification.binding.version===model):undefined;
    if(model&&!qualification)throw new Error("private_choose_context");
    const value:ActorChatValue={...structuredClone(agentSelection),mcp_metadata_ids:[...mcpSelected],expected_mcp_revisions:Object.fromEntries(Object.entries(agentSelection.expected_mcp_revisions).filter(([id])=>mcpSelected.includes(id))),model_profile_id:profile,provider_revision:provider.revision,model_binding:qualification?.qualification.binding??null,output:output??null};
    if(JSON.stringify(value)===JSON.stringify(chatConfig.value)&&!chatLatch.current.pending)return chatConfig;
    const latch=chatLatch.current,request=latch.prepare(()=>({idempotency_key:crypto.randomUUID(),expected_revision:chatConfig.revision,value}));
    const saved=await actorChatConfiguration(client.base,session,project,thread,request);
    if(epoch!==generation.current||scope!==context.current)throw new Error("private_context_changed");
    latch.confirm();setChatConfig(saved);
    if(JSON.stringify(request.value)!==JSON.stringify(value))throw new Error("private_chat_original_save_confirmed_save_current_draft_again");
    return saved;
  }
  async function refreshTasks(){const scope=context.current,epoch=generation.current;if(!project)return;const values=await api<ReleasedTask[]>("/tasks?project_id="+encodeURIComponent(project));if(epoch!==generation.current||scope!==context.current)return;if(values.some(task=>task.project_id!==project||!githubUuid(task.id)||!Number.isSafeInteger(task.revision)||task.revision<1))throw new Error("private_context_changed");setTasks(values.filter(task=>task.released));}
  useEffect(()=>{const scope=context.current;let canceled=false;setTasks([]);setSelectedTask(original?.plan.project_id===project?original.plan.task?.id??"":"");if(project)void refreshTasks().catch(e=>{if(!canceled&&scope===context.current)setError(message(e));});return()=>{canceled=true;};},[client.base,session,actorId,project]);
  async function start(text:string,epoch:number){
    if(invalidOriginal)throw new Error("private_original_invalid");
    const scope=context.current;
    const digest=await sha(text);
    if(epoch!==generation.current||scope!==context.current)throw new Error("private_context_changed");
    const held=pending.current??(original?{original,prompt:text}:undefined);
    if(held){
      if(held.original.prompt_sha256!==digest||held.original.plan.project_id!==project||held.original.plan.thread_id!==thread||held.original.plan.model_profile_id!==profile)throw new Error("private_original_pending");
      pending.current={original:held.original,prompt:text};
      const run=await api<DeviceRun>("/runs","POST",{...held.original.plan,input:{...held.original.plan.input,prompt:text},idempotency_key:held.original.key});
      if(epoch!==generation.current||scope!==context.current)return;
      if(run.spec.project_id!==project||run.spec.thread_id!==thread||run.spec.executor_target.kind!=="server_linux"||!githubUuid(run.spec.id))throw new Error("private_context_changed");
      localStorage.removeItem(journalKey);pending.current=undefined;setOriginal(undefined);setSelectedRun(run.spec.id);if(prompt.current)prompt.current.value="";await refreshRuns(epoch);setNotice(tr("Originaler privater Lauf bestätigt.","Original private run confirmed."));return;
    }
    const p=projects.find(v=>v.id===project),t=threads.find(v=>v.id===thread),provider=profiles.find(v=>v.id===profile&&(v.has_secret||v.model_defaults.auth_mode==="none"));
    if(!p||!t||!provider||!text.trim())throw new Error("private_choose_context");
    if(!agentSelection)throw new Error("private_agent_settings_unavailable");
    const qualification=model?models.find(v=>v.qualification.binding.id+":"+v.qualification.binding.version===model):undefined;
    if(model&&!qualification)throw new Error("private_choose_context");
    const savedConfig=await saveChatConfiguration(epoch);
    if(epoch!==generation.current||scope!==context.current)throw new Error("private_context_changed");
    const expected_mcp_revisions=Object.fromEntries(Object.entries(agentSelection.expected_mcp_revisions).filter(([id])=>mcpSelected.includes(id)));
    const input:RunPlan["input"]={expected_chat_configuration_revision:savedConfig.revision,run_configuration:validateActorRunConfiguration(agentSelection.run_configuration),expected_provider_revision:provider.revision,...(Object.keys(expected_mcp_revisions).length?{expected_mcp_revisions}:{}),...(agentSelection.skill_ids.length?{skill_ids:[...agentSelection.skill_ids]}:{}),...(agentSelection.memory_ids.length?{memory_ids:[...agentSelection.memory_ids]}:{}),...(mcpSelected.length?{mcp_metadata_ids:[...mcpSelected]}:{}),project_file_ids:[],...(github?{github_binding_id:github}:{}),...(qualification?{model_binding:qualification.qualification.binding}:{}),...(output?{output}:{})};
    const task=selectedTask?tasks.find(value=>value.id===selectedTask):undefined;if(selectedTask&&!task)throw new Error("private_choose_context");
    const plan:RunPlan={thread_id:thread,project_id:project,project_revision:p.revision,project_privacy:"private_local",task:task?{id:task.id,revision:task.revision}:null,executor_target:{kind:"server_linux"},required_capabilities:[],model_profile_id:profile,snapshot_id:null,input};
    const recorded={format:1 as const,key:crypto.randomUUID(),plan,prompt_sha256:digest};
    pending.current={original:recorded,prompt:text};localStorage.setItem(journalKey,JSON.stringify(recorded));setOriginal(recorded);
    const run=await api<DeviceRun>("/runs","POST",{...recorded.plan,input:{...recorded.plan.input,prompt:text},idempotency_key:recorded.key});
    if(epoch!==generation.current||scope!==context.current)return;
    if(run.spec.project_id!==project||run.spec.thread_id!==thread||run.spec.executor_target.kind!=="server_linux"||!githubUuid(run.spec.id))throw new Error("private_context_changed");
    localStorage.removeItem(journalKey);pending.current=undefined;setOriginal(undefined);setSelectedRun(run.spec.id);if(prompt.current)prompt.current.value="";await refreshRuns(epoch);setNotice(tr("Originaler privater Lauf bestätigt.","Original private run confirmed."));
  }
  async function newProject(e:FormEvent<HTMLFormElement>){e.preventDefault();const form=e.currentTarget,name=String(new FormData(form).get("name")??"");await attempt(async epoch=>{const value=await api<PrivateGithubProject>("/projects","POST",{name,description:"",privacy:"private_local",team_id:null,preferred_executor_target:{kind:"server_linux"},policy:{},idempotency_key:crypto.randomUUID()});const p=await privateGithubProjects(client,session);if(epoch===generation.current){setProjects(p);setProject(value.id);setThread("");setGithub("");setMcpSelected([]);setModel("");form.reset();}});}
  async function newThread(e:FormEvent<HTMLFormElement>){e.preventDefault();const form=e.currentTarget,title=String(new FormData(form).get("title")??"");await attempt(async epoch=>{const scope=context.current;const value=await api<Thread>("/threads","POST",{project_id:project,title,forked_from_thread_id:null,forked_from_message_id:null});if(epoch===generation.current&&scope===context.current){setThreads(v=>[...v,value]);setThread(value.id);form.reset();}});}
  async function saveProfile(plan:ProfilePlan,token:string,epoch:number){
      if(invalidProfileOriginal)throw new Error("private_original_invalid");
      localStorage.setItem(profileJournalKey,JSON.stringify(plan));profilePending.current={plan,token};setProfileOriginal(plan);
      const response=await fetch(client.base+"/api/v1/provider-profiles",{method:"POST",headers:{Authorization:"Bearer "+session,"Content-Type":"application/json","Idempotency-Key":plan.key},credentials:"omit",cache:"no-store",redirect:"error",body:JSON.stringify({...plan.body,api_key:plan.body.model_defaults.auth_mode==="none"?null:token})});
      if(!response.ok)throw new Error("private_profile_save_unconfirmed");
      const value=await response.json() as Profile;if(value.owner_user_id!==actorId||value.team_id!==null||!githubUuid(value.id))throw new Error("private_context_changed");
      if(epoch!==generation.current)return;localStorage.removeItem(profileJournalKey);profilePending.current=undefined;setProfileOriginal(undefined);
      const actual=await api<Profile[]>("/provider-profiles");if(epoch===generation.current){setProfiles(actual);setProfile(value.id);setModel("");setNotice(tr("Eigenes Profil gespeichert.","Own profile saved."));}
  }
  async function newProfile(e:FormEvent<HTMLFormElement>){
    e.preventDefault();const form=e.currentTarget,data=new FormData(form),auth=String(data.get("auth"))==="none"?"none" as const:"bearer" as const,token=String(data.get("key")??"");form.reset();
    const plan=profileOriginal??{format:1 as const,key:crypto.randomUUID(),body:{team_id:null,name:String(data.get("name")??""),provider_kind:"openai_compatible" as const,model_defaults:{base_url:String(data.get("url")??""),model:String(data.get("model")??""),context_tokens:Number(data.get("context")),auth_mode:auth}}};
    await attempt(epoch=>saveProfile(plan,token,epoch));
  }
  return <section className="product-stack"><h2>{tr("Private Serverarbeit","Private server work")}</h2><p>{tr("Diese Projekte, Gespräche und Zugänge gehören deinem Konto. Wähle das eigene Serverprofil und optional einen GitHub-Zugang für jeden neuen Lauf.","These projects, conversations and credentials belong to your account. Select your own server profile and optionally a GitHub credential for each new run.")}</p>{error&&<p className="error" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
    <label>{tr("Privates Projekt","Private project")}<select value={project} disabled={busy||!!original} onChange={e=>{setProject(e.target.value);setThread("");setGithub("");setMcpSelected([]);setModel("");}}><option value="">{tr("Auswählen","Select")}</option>{projects.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
    {!original&&<form onSubmit={e=>void newProject(e)}><label>{tr("Neues privates Projekt","New private project")}<input name="name" required maxLength={200}/></label><button disabled={busy}>{tr("Privates Projekt anlegen","Create private project")}</button></form>}
    <label>{tr("Gespräch","Conversation")}<select value={thread} disabled={busy||!!original||!project} onChange={e=>setThread(e.target.value)}><option value="">{tr("Auswählen","Select")}</option>{threads.map(v=><option key={v.id} value={v.id}>{v.title}</option>)}</select></label>
    {project&&!original&&<form onSubmit={e=>void newThread(e)}><label>{tr("Neues Gespräch","New conversation")}<input name="title" required maxLength={200}/></label><button disabled={busy}>{tr("Gespräch anlegen","Create conversation")}</button></form>}
    <PrivateActorSchedulesPanel key={JSON.stringify(["PrivateActorSchedulesPanel",client.base,actorId,project])} client={client} session={session} actorId={actorId} projectId={project||undefined}/>
    {project&&thread&&<PrivateActorDeviceInterop key={JSON.stringify(["PrivateActorDeviceInterop",client.base,actorId,project,thread])} endpoint={client.base} token={session} actorId={actorId} projectId={project} orderId={thread}/>}
    {project&&<AndroidPrivateSharingPanel key={JSON.stringify(["AndroidPrivateSharingPanel",client.base,actorId,project])} endpoint={client.base} token={session} projectId={project}/>}
    {project&&<AndroidAdministrationControlPanel key={JSON.stringify(["AndroidAdministrationControlPanel",client.base,actorId,project,"control"])} endpoint={client.base} token={session} projectId={project}/>}
    {project&&<AndroidMetadataSyncPanel key={JSON.stringify(["AndroidMetadataSyncPanel",client.base,actorId,project,"sync"])} endpoint={client.base} token={session} projectId={project}/>}
    <AndroidMobileCachePanel key={JSON.stringify(["AndroidMobileCachePanel",client.base,actorId])} endpoint={client.base} token={session} actorId={actorId} threadId={thread||undefined} onChanged={()=>void refreshRuns()}/>
    {project&&<AndroidProjectVersionsPanel key={JSON.stringify(["AndroidProjectVersionsPanel",client.base,actorId,project])} endpoint={client.base} token={session} actorId={actorId} projectId={project}/>}
    {project&&<PrivateWorkflowRegistryPanel key={JSON.stringify(["PrivateWorkflowRegistryPanel",client.base,actorId,project])} endpoint={client.base} token={session} actorId={actorId} projectId={project}/>}
    <label>{tr("Eigenes Serverprofil","Own server profile")}<select value={profile} disabled={busy||!!original} onChange={e=>{setProfile(e.target.value);setProfileRevision(undefined);setModel("");}}><option value="">{tr("Auswählen","Select")}</option>{profiles.filter(p=>p.has_secret||p.model_defaults.auth_mode==="none").map(p=><option key={p.id} value={p.id}>{p.name} · {p.model_defaults.model}</option>)}</select></label>
    {session&&githubUuid(actorId)&&chatConfigReady&&<ActorAgentConfigurationPanel key={JSON.stringify(["ActorAgentConfigurationPanel",client.base,session,actorId,project,thread])} client={client} session={session} actorId={actorId} providers={profiles} profilesReady={profilesReady} selectedProfile={profile} initialSelection={restoredSelection} disabled={busy||!!original} onProfile={id=>{setProfile(id);setProfileRevision(undefined);setModel("");}} onSelection={setAgentSelection} onMcpSelection={setMcpSelected} onCodexProfile={p=>{setProfiles(current=>[...current.filter(item=>item.id!==p.id),{id:p.id,owner_user_id:actorId,team_id:null,name:p.name,revision:p.revision,has_secret:false,model_defaults:{model:p.model,context_tokens:p.context_tokens,auth_mode:"none"}}]);}}/>}
    {thread&&!original&&<p><button type="button" disabled={busy||!chatConfigReady||!agentSelection} onClick={()=>void attempt(async epoch=>{await saveChatConfiguration(epoch);setNotice(tr("Konfiguration als unabhängige Kopie für dieses Gespräch gespeichert.","Configuration saved as an independent copy for this conversation."));})}>{tr(chatLatch.current.pending?"Ursprüngliche Speicherung abgleichen":"Gesprächskonfiguration speichern",chatLatch.current.pending?"Reconcile original save":"Save conversation configuration")}</button> {chatConfig?.revision?tr("Gespeicherte Revision: ","Saved revision: ")+chatConfig.revision:""}</p>}
    {project&&thread&&agentSelection&&<ActorContextPanel key={JSON.stringify(["ActorContextPanel",client.base,actorId,project,thread])} client={client} session={session} actorId={actorId} project={project} disabled={busy||!!original} skillIds={agentSelection.skill_ids} memoryIds={agentSelection.memory_ids} memoryAvailable={memoryAvailable} onSelection={(kind,ids)=>setAgentSelection(current=>current?{...current,...(kind==="skills"?{skill_ids:ids}:{memory_ids:ids}),run_configuration:{...current.run_configuration,agent_config:{...current.run_configuration.agent_config,...(kind==="skills"?{skill_ids:ids}:{memory_ids:ids})}}}:current)}/>}
    {project&&<ExternalMemoryPanel client={client} session={session} actorId={actorId} project={project} disabled={busy||!!original}/>}
    {!original&&<details className="product-item"><summary>{tr("Eigenes Modellprofil einrichten","Configure own model profile")}</summary>{profileOriginal&&<p role="status">{tr("Eine Profilbestätigung fehlt. Verwende denselben ursprünglichen Schlüssel und Zugang.","A profile acknowledgement is missing. Use the same original operation and credential.")}</p>}
      {profilePending.current&&<button disabled={busy} onClick={()=>void attempt(epoch=>{const p=profilePending.current!;return saveProfile(p.plan,p.token,epoch);})}>{tr("Originales Profil erneut abrufen","Retrieve original profile again")}</button>}
      <form key={profileOriginal?.key??"new-profile"} onSubmit={e=>void newProfile(e)}><label>{tr("Profilname","Profile name")}<input name="name" required maxLength={200} defaultValue={profileOriginal?.body.name} disabled={!!profileOriginal}/></label><label>{tr("Modellserver-URL","Model server URL")}<input name="url" type="url" required placeholder="https://litellm.example/v1" defaultValue={profileOriginal?.body.model_defaults.base_url} disabled={!!profileOriginal}/></label><label>{tr("Modellname","Model name")}<input name="model" required maxLength={200} defaultValue={profileOriginal?.body.model_defaults.model} disabled={!!profileOriginal}/></label><label>{tr("Kontexttokens","Context tokens")}<input name="context" type="number" min={1024} max={2000000} defaultValue={profileOriginal?.body.model_defaults.context_tokens??32768} required disabled={!!profileOriginal}/></label><label>{tr("Authentifizierung","Authentication")}<select name="auth" defaultValue={profileOriginal?.body.model_defaults.auth_mode??"bearer"} disabled={!!profileOriginal}><option value="bearer">Bearer</option><option value="none">{tr("Ohne Zugangsschlüssel","No credential")}</option></select></label><label>{tr("Eigener API-Schlüssel","Own API key")}<input name="key" type="password" maxLength={4096} autoComplete="off" disabled={!!profilePending.current}/></label><button disabled={busy||!!profilePending.current||invalidProfileOriginal}>{profileOriginal?tr("Originalprofil mit gleichem Schlüssel abrufen","Retrieve original profile with the same credential"):tr("Eigenes Profil verschlüsselt speichern","Save encrypted own profile")}</button></form>
      {(profileOriginal||invalidProfileOriginal)&&<button disabled={busy} onClick={()=>{localStorage.removeItem(profileJournalKey);setInvalidProfileOriginal(false);profilePending.current=undefined;setProfileOriginal(undefined);setNotice(tr("Lokale Profilreferenz verworfen. Vor einem neuen Auftrag den aktuellen Profilstand prüfen.","Local profile reference discarded. Review current profiles before a new request."));}}>{tr("Lokale Profilreferenz ausdrücklich verwerfen","Explicitly discard local profile reference")}</button>}
    </details>}
    <label>{tr("Qualifizierte Modellreferenz","Qualified model reference")}<select value={model} disabled={busy||!!original||!profile} onChange={e=>setModel(e.target.value)}><option value="">{tr("Konservative Kontextbegrenzung","Conservative context limit")}</option>{models.map(row=><option key={row.qualification.binding.id+":"+row.qualification.binding.version} value={row.qualification.binding.id+":"+row.qualification.binding.version}>{row.qualification.binding.id} · v{row.qualification.binding.version}</option>)}</select></label>
    {project&&<PrivateGithubBindingPanel key={JSON.stringify(["PrivateGithubBindingPanel",client.base,actorId,project])} client={client} session={session} actorId={actorId} projectId={project} selectedBindingId={github} selectionDisabled={busy||!!original} onSelect={(p,id)=>{if(p===project&&!busy&&!original)setGithub(id);}}/>}
    {project&&<PrivateMcpBindingsPanel key={JSON.stringify(["PrivateMcpBindingsPanel",client.base,actorId,project])} client={client} session={session} actorId={actorId} projectId={project} selected={mcpSelected} disabled={busy||!!original} onSelect={ids=>{if(!busy&&!original)setMcpSelected(ids);}}/>}
    {thread&&<form onSubmit={e=>{e.preventDefault();void attempt(epoch=>start(prompt.current?.value??"",epoch));}}><label>{tr("Veröffentlichte Taskvorlage","Published task template")}<select value={selectedTask} disabled={busy||!!original} onChange={e=>setSelectedTask(e.target.value)}><option value="">{tr("Freier Modellauftrag","Free model task")}</option>{selectedTask&&!tasks.some(task=>task.id===selectedTask)&&<option value={selectedTask}>{tr("Gespeicherte Originalvorlage","Recorded original template")} · {selectedTask}</option>}{tasks.map(task=><option key={task.id} value={task.id}>{task.name} · v{task.revision}</option>)}</select></label><button type="button" disabled={busy||!!original} onClick={()=>void attempt(()=>refreshTasks())}>{tr("Aktuelle Taskvorlagen laden","Reload published templates")}</button><label>{tr("Auftrag","Task")}<textarea ref={prompt} required maxLength={131072} rows={5}/></label><StructuredOutputEditor value={output} onChange={setOutput} disabled={busy||!!original}/><button disabled={busy||!profile||invalidOriginal||!original&&!agentSelection}>{original?tr("Originallauf mit ursprünglichem Text abrufen","Retrieve original run with original text"):tr("Privaten Serverlauf starten","Start private server run")}</button></form>}
    {(original||invalidOriginal)&&<aside><p>{tr("Der Originalauftrag bleibt gespeichert. Auftragstext und Zugangsschlüssel werden nicht dauerhaft im Browser gespeichert.","The original request is retained. Task text and credentials are not stored persistently in the browser.")}</p>{pending.current&&<button disabled={busy} onClick={()=>void attempt(epoch=>start(pending.current!.prompt,epoch))}>{tr("Originalauftrag erneut abrufen","Retrieve original request again")}</button>}<button disabled={busy} onClick={()=>{localStorage.removeItem(journalKey);pending.current=undefined;setOriginal(undefined);setInvalidOriginal(false);setError("");setNotice(tr("Lokale Referenz verworfen. Frühere Läufe und unklare Wirkungen bleiben erhalten.","Local reference discarded. Earlier runs and uncertain effects remain retained."));}}>{tr("Lokale Referenz ausdrücklich verwerfen","Explicitly discard local reference")}</button></aside>}
    <label>{tr("Laufhistorie","Run history")}<select value={selectedRun} onChange={e=>setSelectedRun(e.target.value)}><option value="">{tr("Lauf auswählen","Select a run")}</option>{runs.map(r=><option key={r.spec.id} value={r.spec.id}>{r.state} · {r.spec.id}</option>)}</select></label>
    {runs.filter(r=>r.spec.id===selectedRun).map(run=><article key={run.spec.id} className="product-item"><h3>{run.state}</h3>{["running","queued","waiting_for_approval","waiting_for_input"].includes(run.state)&&<button disabled={busy} onClick={()=>void attempt(async epoch=>{await api(`/runs/${run.spec.id}/cancel`,"POST",{});await refreshRuns(epoch);})}>{tr("Diesen Lauf abbrechen","Cancel this run")}</button>}<DeviceRunRequests endpoint={client.base} token={session} runId={run.spec.id} target="server"/><DeviceRunResult endpoint={client.base} token={session} run={run}/></article>)}
    {selectedRun&&<McpRunTasksPanel key={JSON.stringify(["McpRunTasksPanel",client.base,actorId,selectedRun])} client={client} session={session} realm="personal" run={selectedRun}/>}
    {project&&selectedRun&&runs.some(run=>run.spec.id===selectedRun&&run.spec.executor_target.kind==="server_linux"&&["workflow","crew"].includes(String((run.spec.input as Record<string,unknown>).task_runner)))&&<PrivateWorkflowRunPanel key={JSON.stringify(["PrivateWorkflowRunPanel",client.base,actorId,project,selectedRun])} endpoint={client.base} token={session} actorId={actorId} projectId={project} runId={selectedRun}/>}
    {selectedRun&&<AndroidRunRecoveryPanel client={client} session={session} actorId={actorId} runId={selectedRun} onChanged={()=>refreshRuns()}/>}
  </section>;
}
