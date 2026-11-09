import {defaultRunBudgets, type AccessMode, type Budgets, type Client, type McpToolSelection, type ProviderProfile, type WorkflowDefinition, type WorkflowRole} from "@cowork/client";
import {PersistentRequestLatch,requestJournalEndpoint,type RequestJournalStorage} from "./requestJournal";

export type GenerationSettings={temperature?:number;timeout_ms?:number};
export type AgentModelSelection={kind:"inherit"}|{kind:"provider";provider_id:string;provider_revision:number}|{kind:"codex";account_id:string;model:string|null;reasoning_effort:string|null};
export type PersonalityDefinition={name:string;description:string;icon:string|null;role:string;goal:string;system_prompt:string;skills_markdown:string;generation:GenerationSettings;model:AgentModelSelection;is_default:boolean};
export type LibraryRecord<T>={id:string;revision:number;created_at:number;updated_at:number;value:T};
export type Personality=LibraryRecord<PersonalityDefinition>;
export type LibrarySource={id:string;revision:number};
export type EffectiveAgentConfig={instructions:string;generation:GenerationSettings;execution_mode?:import("@cowork/client").AgentExecutionMode;behavior?:import("@cowork/client").AgentBehaviorSettings;access_mode?:AccessMode;tool_filter?:string[];skill_ids?:string[];memory_ids?:string[];personality_source?:LibrarySource;agent_source?:LibrarySource};
export type AgentConfiguration={role:string;goal:string;backstory?:string;instructions:string;skills_markdown:string;model:AgentModelSelection;generation:GenerationSettings;access_mode:AccessMode;budgets:Budgets;native_tools:string[];mcp_tools:McpToolSelection[];skill_ids:string[];memory_ids:string[];allow_delegation:boolean;tool_filter?:string[];personality_source?:LibrarySource;agent_source?:LibrarySource};
export type AgentTemplateDefinition={name:string;description:string;icon:string|null;personality:Personality|null;configuration:AgentConfiguration};
export type AgentTemplate=LibraryRecord<AgentTemplateDefinition>;
export type LibraryKind="personalities"|"agent-templates";
export type LibraryDefinition=PersonalityDefinition|AgentTemplateDefinition;
export type LibraryItem=Personality|AgentTemplate;
export type LibraryContext={client:Client;session?:string;actorId?:string};
export type LibraryWrite={idempotency_key:string;expected_revision:number;value:LibraryDefinition};
export type LibraryMutation={kind:LibraryKind;id?:string;method:"POST"|"PUT"|"DELETE";body:LibraryWrite|{idempotency_key:string;expected_revision:number}};
export class LibraryError extends Error{constructor(readonly code:string,readonly status=0){super(code);}}

export function libraryIdentity({client,session,actorId}:LibraryContext):string {
  if(session!==undefined||actorId!==undefined){if(!session||!actorId)throw new LibraryError("library_actor_session_required");return JSON.stringify([client.base,"actor",actorId,session]);}
  if(!client.token)throw new LibraryError("library_owner_session_required");
  return JSON.stringify([client.base,"owner",client.token]);
}
export const libraryQueryKey=(context:LibraryContext,kind:LibraryKind)=>["agent-library",libraryIdentity(context),kind] as const;
const object=(value:unknown):value is Record<string,unknown>=>!!value&&typeof value==="object"&&!Array.isArray(value);
function onlyKeys(value:Record<string,unknown>,allowed:string[]){if(Object.keys(value).some(key=>!allowed.includes(key)))throw new LibraryError("library_invalid_definition");}
const strings=(value:unknown):value is string[]=>Array.isArray(value)&&value.every(item=>typeof item==="string");
const finite=(value:unknown)=>typeof value==="number"&&Number.isFinite(value);
const positive=(value:unknown)=>Number.isSafeInteger(value)&&Number(value)>0;
function model(value:unknown):value is AgentModelSelection {
  if(object(value))onlyKeys(value,value.kind==="provider"?["kind","provider_id","provider_revision"]:value.kind==="codex"?["kind","account_id","model","reasoning_effort"]:["kind"]);
  return object(value)&&(value.kind==="inherit"||value.kind==="provider"&&typeof value.provider_id==="string"&&!!value.provider_id&&positive(value.provider_revision)||value.kind==="codex"&&typeof value.account_id==="string"&&(value.model===null||typeof value.model==="string")&&(value.reasoning_effort===null||typeof value.reasoning_effort==="string"));
}
function generation(value:unknown):value is GenerationSettings{if(object(value))onlyKeys(value,["temperature","timeout_ms"]);return object(value)&&(value.temperature===undefined||finite(value.temperature)&&Number(value.temperature)>=0&&Number(value.temperature)<=2)&&(value.timeout_ms===undefined||positive(value.timeout_ms)&&Number(value.timeout_ms)>=1000&&Number(value.timeout_ms)<=1200000);}
function metadata(value:unknown):value is LibraryRecord<unknown>{return object(value)&&typeof value.id==="string"&&!!value.id&&positive(value.revision)&&finite(value.created_at)&&finite(value.updated_at)&&object(value.value);}
export function validatePersonality(value:unknown):PersonalityDefinition {
  if(object(value))onlyKeys(value,["name","description","icon","role","goal","system_prompt","skills_markdown","generation","model","is_default"]);
  if(!object(value)||!["name","description","role","goal","system_prompt","skills_markdown"].every(key=>typeof value[key]==="string")||!String(value.name).trim()||!String(value.system_prompt).trim()||!(value.icon===null||typeof value.icon==="string")||typeof value.is_default!=="boolean"||!generation(value.generation)||!model(value.model))throw new LibraryError("library_invalid_definition");
  return value as PersonalityDefinition;
}
export function validateAgentTemplate(value:unknown):AgentTemplateDefinition {
  if(object(value))onlyKeys(value,["name","description","icon","personality","configuration"]);
  if(!object(value)||typeof value.name!=="string"||!value.name.trim()||typeof value.description!=="string"||!(value.icon===null||typeof value.icon==="string")||!object(value.configuration))throw new LibraryError("library_invalid_definition");
  if(value.personality!==null){if(!metadata(value.personality))throw new LibraryError("library_invalid_definition");onlyKeys(value.personality,["id","revision","created_at","updated_at","value"]);validatePersonality(value.personality.value);}
  const c=value.configuration;
  onlyKeys(c,["role","goal","backstory","instructions","skills_markdown","model","generation","access_mode","budgets","native_tools","mcp_tools","skill_ids","memory_ids","allow_delegation","personality_source","agent_source","tool_filter"]);
  if(c.backstory!==undefined&&typeof c.backstory!=="string")throw new LibraryError("library_invalid_definition");
  if(c.tool_filter!==undefined&&!strings(c.tool_filter))throw new LibraryError("library_invalid_definition");
  if(object(c.budgets))onlyKeys(c.budgets,["max_steps","max_seconds","max_output_tokens","max_total_tokens"]);
  for(const source of [c.personality_source,c.agent_source])if(source!==undefined){if(!object(source)||typeof source.id!=="string"||!positive(source.revision))throw new LibraryError("library_invalid_definition");onlyKeys(source,["id","revision"]);}
  if(Array.isArray(c.mcp_tools))for(const tool of c.mcp_tools)if(object(tool))onlyKeys(tool,["server_id","server_revision","tool_name"]);
  if(!["role","goal","instructions","skills_markdown"].every(key=>typeof c[key]==="string")||!String(c.instructions).trim()||!model(c.model)||!generation(c.generation)||!["read_only","confirm_mutations","confirm_all","autonomous"].includes(String(c.access_mode))||!object(c.budgets)||!["max_steps","max_seconds","max_output_tokens"].every(key=>positive((c.budgets as Record<string,unknown>)[key]))||!(c.budgets.max_total_tokens===undefined||positive(c.budgets.max_total_tokens))||!strings(c.native_tools)||!strings(c.skill_ids)||!strings(c.memory_ids)||!Array.isArray(c.mcp_tools)||!c.mcp_tools.every(tool=>object(tool)&&typeof tool.server_id==="string"&&positive(tool.server_revision)&&typeof tool.tool_name==="string")||typeof c.allow_delegation!=="boolean")throw new LibraryError("library_invalid_definition");
  return value as AgentTemplateDefinition;
}
export function validateLibraryItem(kind:LibraryKind,value:unknown):LibraryItem {
  if(!metadata(value))throw new LibraryError("library_invalid_response");
  (kind==="personalities"?validatePersonality:validateAgentTemplate)(value.value);
  return value as LibraryItem;
}
async function libraryFetch(context:LibraryContext,path:string,method="GET",body?:unknown):Promise<unknown>{
  libraryIdentity(context);
  const actor=context.session!==undefined;
  const response=await fetch(context.client.base+(actor?"/api/v1":"/api/v2")+path,{method,headers:{Authorization:"Bearer "+(actor?context.session:context.client.token),...(body===undefined?{}:{"Content-Type":"application/json"})},body:body===undefined?undefined:JSON.stringify(body),credentials:"omit",cache:"no-store",redirect:"error"});
  if(!response.ok){let code="library_request_failed";try{const raw:unknown=await response.json();if(object(raw)&&typeof(raw.code??raw.error)==="string")code=String(raw.code??raw.error);}catch{/* Preserve the HTTP failure. */}throw new LibraryError(code,response.status);}
  return await response.json();
}
export function libraryRequest(context:LibraryContext,kind:LibraryKind,id?:string,method="GET",body?:unknown){return libraryFetch(context,"/library/"+kind+(id?"/"+encodeURIComponent(id):""),method,body);}
export type LibraryPreferences={revision:number;updated_at:number;value:Record<string,unknown>&{default_agent_id?:string|null;default_personality_id?:string|null}};
export type LibraryPreferencesWrite={idempotency_key:string;expected_revision:number;value:LibraryPreferences["value"]};
function preferencesRecord(raw:unknown):LibraryPreferences{if(!object(raw)||!Number.isSafeInteger(raw.revision)||Number(raw.revision)<0||!finite(raw.updated_at)||!object(raw.value))throw new LibraryError("library_invalid_response");return raw as LibraryPreferences;}
export async function libraryPreferences(context:LibraryContext):Promise<LibraryPreferences>{return preferencesRecord(await libraryFetch(context,"/preferences"));}
export async function writeLibraryPreferences(context:LibraryContext,request:LibraryPreferencesWrite):Promise<LibraryPreferences>{const result=preferencesRecord(await libraryFetch(context,"/preferences","PUT",request));if(result.revision!==request.expected_revision+1||result.value.default_agent_id!==request.value.default_agent_id)throw new LibraryError("library_invalid_response");return result;}
export async function listLibrary(context:LibraryContext,kind:LibraryKind):Promise<LibraryItem[]>{const raw=await libraryRequest(context,kind);if(!Array.isArray(raw)||raw.length>1000)throw new LibraryError("library_invalid_response");return raw.map(value=>validateLibraryItem(kind,value));}
export async function mutateLibrary(context:LibraryContext,mutation:LibraryMutation):Promise<LibraryItem|{id:string;revision:number;deleted:true}>{
  const raw=await libraryRequest(context,mutation.kind,mutation.id,mutation.method,mutation.body);
  if(mutation.method==="DELETE"){
    if(!object(raw)||raw.id!==mutation.id||raw.revision!==mutation.body.expected_revision+1||raw.deleted!==true)throw new LibraryError("library_invalid_response");
    return raw as {id:string;revision:number;deleted:true};
  }
  const item=validateLibraryItem(mutation.kind,raw);
  if(mutation.id&&item.id!==mutation.id||item.revision!==mutation.body.expected_revision+1)throw new LibraryError("library_invalid_response");
  return item;
}
/** A retry must preserve the original request, even if the editor has changed. */
function noCredentialFields(value:unknown):void{if(Array.isArray(value)){for(const item of value)noCredentialFields(item);}else if(object(value)){if(Object.keys(value).some(key=>/^(?:api_key|apikey|password|secret|access_token|refresh_token|authorization)$/i.test(key)))throw new LibraryError("library_journal_invalid");for(const item of Object.values(value))noCredentialFields(item);}}
function journalMutation(raw:unknown):LibraryMutation{
  noCredentialFields(raw);
  if(!object(raw)||!["personalities","agent-templates"].includes(String(raw.kind))||!["POST","PUT","DELETE"].includes(String(raw.method))||!object(raw.body)||typeof raw.body.idempotency_key!=="string"||!raw.body.idempotency_key||!Number.isSafeInteger(raw.body.expected_revision)||Number(raw.body.expected_revision)<0||raw.id!==undefined&&typeof raw.id!=="string")throw new LibraryError("library_journal_invalid");
  onlyKeys(raw,["kind","id","method","body"]);onlyKeys(raw.body,raw.method==="DELETE"?["idempotency_key","expected_revision"]:["idempotency_key","expected_revision","value"]);
  if(raw.method!=="DELETE")(raw.kind==="personalities"?validatePersonality:validateAgentTemplate)(raw.body.value);
  return raw as LibraryMutation;
}
export function libraryStorageKey(context:LibraryContext,suffix:string):string{libraryIdentity(context);return "cowork.agent-library.pending.v1:"+JSON.stringify([requestJournalEndpoint(context.client.base),context.actorId??"installation-owner",suffix]);}
export class LibraryMutationLatch extends PersistentRequestLatch<LibraryMutation>{constructor(key?:string,storage?:RequestJournalStorage){super(key,journalMutation,storage);}}
export class LibraryPreferencesLatch extends PersistentRequestLatch<LibraryPreferencesWrite>{constructor(key?:string,storage?:RequestJournalStorage){super(key,raw=>{noCredentialFields(raw);if(!object(raw)||typeof raw.idempotency_key!=="string"||!Number.isSafeInteger(raw.expected_revision)||Number(raw.expected_revision)<0||!object(raw.value))throw new LibraryError("library_journal_invalid");onlyKeys(raw,["idempotency_key","expected_revision","value"]);return raw as LibraryPreferencesWrite;},storage);}}
export function emptyPersonality():PersonalityDefinition{return {name:"",description:"",icon:null,role:"custom",goal:"",system_prompt:"",skills_markdown:"",generation:{},model:{kind:"inherit"},is_default:false};}
export function emptyAgentTemplate():AgentTemplateDefinition{return {name:"",description:"",icon:null,personality:null,configuration:{role:"custom",goal:"",backstory:"",instructions:"",skills_markdown:"",model:{kind:"inherit"},generation:{},access_mode:"confirm_mutations",budgets:{...defaultRunBudgets},native_tools:[],mcp_tools:[],skill_ids:[],memory_ids:[],allow_delegation:false}};}
export function personalityInstructions(value:PersonalityDefinition):string{return [value.role&&`Role: ${value.role}`,value.goal&&`Goal: ${value.goal}`,value.system_prompt,value.skills_markdown&&`Skills:\n${value.skills_markdown}`].filter(Boolean).join("\n\n");}
export function agentInstructions(value:AgentTemplateDefinition):string{return [value.personality&&personalityInstructions(value.personality.value),value.configuration.role&&`Role: ${value.configuration.role}`,value.configuration.goal&&`Goal: ${value.configuration.goal}`,value.configuration.backstory&&`Backstory: ${value.configuration.backstory}`,value.configuration.instructions,value.configuration.skills_markdown&&`Skills:\n${value.configuration.skills_markdown}`].filter(Boolean).join("\n\n");}
export function effectivePersonality(personality:Personality):EffectiveAgentConfig{return {instructions:personalityInstructions(personality.value),generation:{...personality.value.generation},personality_source:{id:personality.id,revision:personality.revision}};}
export function resolveAgentModel(item:AgentTemplate):AgentModelSelection{return item.value.configuration.model.kind==="inherit"?item.value.personality?.value.model??{kind:"inherit"}:item.value.configuration.model;}
export function effectiveAgent(item:AgentTemplate):EffectiveAgentConfig{return {instructions:agentInstructions(item.value),generation:{...item.value.personality?.value.generation,...Object.fromEntries(Object.entries(item.value.configuration.generation).filter(([,value])=>value!==undefined))},access_mode:item.value.configuration.access_mode,...(item.value.configuration.skill_ids.length?{skill_ids:[...item.value.configuration.skill_ids]}:{}),...(item.value.configuration.memory_ids.length?{memory_ids:[...item.value.configuration.memory_ids]}:{}),...(item.value.configuration.tool_filter?{tool_filter:[...item.value.configuration.tool_filter]}:{}),...(item.value.personality?{personality_source:{id:item.value.personality.id,revision:item.value.personality.revision}}:{}),agent_source:{id:item.id,revision:item.revision}};}
export function parseLibraryImport(raw:string,kind:LibraryKind):LibraryDefinition {
  if(new TextEncoder().encode(raw).length>1_048_576)throw new LibraryError("library_import_too_large");
  const input:unknown=JSON.parse(raw);
  if(!object(input)||input.format!=="cowork-agent-library/1"||input.kind!==kind)throw new LibraryError("library_import_kind_mismatch");
  const value=structuredClone(kind==="personalities"?validatePersonality(input.value):validateAgentTemplate(input.value));
  // Imports never change the default personality before an explicit save.
  if("is_default" in value)value.is_default=false;
  if("configuration" in value){
    if(value.personality){const snapshot=value.personality.value,c=value.configuration;c.instructions=[personalityInstructions(snapshot),c.instructions].filter(Boolean).join("\n\n");c.generation={...snapshot.generation,...Object.fromEntries(Object.entries(c.generation).filter(([,value])=>value!==undefined))};if(c.model.kind==="inherit")c.model=structuredClone(snapshot.model);value.personality=null;}
    delete value.configuration.personality_source;delete value.configuration.agent_source;
  }
  return value;
}
export function exportLibraryItem(kind:LibraryKind,value:LibraryDefinition):string{return JSON.stringify({format:"cowork-agent-library/1",kind,value},null,2)+"\n";}
export function applyPersonalityToWorkflow(role:WorkflowRole,personality:Personality,providers?:ProviderProfile[]):WorkflowRole {
  const selected=personality.value.model;if(selected.kind==="codex")throw new LibraryError("library_workflow_codex_unavailable");
  const provider=selected.kind==="provider"?providers?.find(p=>p.id===selected.provider_id&&p.revision===selected.provider_revision):null;
  if(selected.kind==="provider"&&!provider)throw new LibraryError("library_provider_changed");
  return {...role,instructions:personalityInstructions(personality.value),agent_config:effectivePersonality(personality),...(provider?{provider_id:provider.id,provider_revision:provider.revision,model_binding:undefined}:{})} as WorkflowRole;
}
export async function mcpLibraryAlias(server:string,name:string):Promise<string>{const bytes=await crypto.subtle.digest("SHA-256",new TextEncoder().encode(JSON.stringify([server,name])));return "mcp_"+Array.from(new Uint8Array(bytes),byte=>byte.toString(16).padStart(2,"0")).join("").slice(0,40);}
export async function applyAgentToWorkflow(definition:WorkflowDefinition,index:number,item:AgentTemplate,providers:ProviderProfile[]):Promise<WorkflowDefinition> {
  const c=item.value.configuration,role=definition.roles[index];if(!role)throw new LibraryError("library_role_missing");
  const effectiveModel=resolveAgentModel(item);
  if(effectiveModel.kind==="codex")throw new LibraryError("library_workflow_codex_unavailable");
  const selected=effectiveModel.kind==="provider"?effectiveModel:null;
  const provider=selected?providers.find(p=>p.id===selected.provider_id&&p.revision===selected.provider_revision):null;
  if(selected&&!provider)throw new LibraryError("library_provider_changed");
  const mcpAliases=await Promise.all(c.mcp_tools.map(tool=>mcpLibraryAlias(tool.server_id,tool.tool_name)));
  const reads=new Set(["read_artifact","list_artifacts","request_input","list_files","read_text_file","stat_file"]);
  const next={...role,instructions:agentInstructions(item.value),agent_config:effectiveAgent(item),...(provider?{provider_id:provider.id,provider_revision:provider.revision,model_binding:undefined}:{}),rights:{...role.rights,tools:[...new Set(c.tool_filter??[...role.rights.tools,...c.native_tools,...mcpAliases])].filter(tool=>definition.rights.tools.includes(tool)&&(c.access_mode!=="read_only"||reads.has(tool))),folders:[...role.rights.folders]},budgets:{max_steps:Math.min(c.budgets.max_steps,definition.budgets.max_steps),max_seconds:Math.min(c.budgets.max_seconds,definition.budgets.max_seconds),max_output_tokens:Math.min(c.budgets.max_output_tokens,definition.budgets.max_output_tokens),max_total_tokens:Math.min(c.budgets.max_total_tokens??defaultRunBudgets.max_total_tokens,definition.budgets.max_total_tokens??defaultRunBudgets.max_total_tokens)},delegate_to:c.allow_delegation?role.delegate_to.filter(id=>definition.roles.some(r=>r.id===id)):[]};
  return {...definition,roles:definition.roles.map((r,i)=>i===index?next:r),steps:definition.steps.map(step=>step.role_id===role.id?{...step,rights:{...step.rights,tools:step.rights.tools.filter(tool=>next.rights.tools.includes(tool)),folders:[...step.rights.folders]},budgets:{max_steps:Math.min(step.budgets.max_steps,next.budgets.max_steps),max_seconds:Math.min(step.budgets.max_seconds,next.budgets.max_seconds),max_output_tokens:Math.min(step.budgets.max_output_tokens,next.budgets.max_output_tokens),max_total_tokens:Math.min(step.budgets.max_total_tokens??defaultRunBudgets.max_total_tokens,next.budgets.max_total_tokens)}}:step)};
}
/** Refresh a recorded source deliberately, retaining the role's current permission ceiling. */
export async function refreshWorkflowRoleFromLibrary(definition:WorkflowDefinition,index:number,item:AgentTemplate|Personality,providers:ProviderProfile[]):Promise<WorkflowDefinition> {
  const original=definition.roles[index];if(!original)throw new LibraryError("library_role_missing");
  const isAgent="configuration" in item.value,source=isAgent?original.agent_config?.agent_source:original.agent_config?.personality_source;
  if(!source||source.id!==item.id||item.revision<source.revision||!isAgent&&original.agent_config?.agent_source)throw new LibraryError("library_source_changed");
  const refreshed=isAgent?await applyAgentToWorkflow(definition,index,item as AgentTemplate,providers):{...definition,roles:definition.roles.map((role,i)=>i===index?applyPersonalityToWorkflow(role,item as Personality,providers):role)};
  const candidate=refreshed.roles[index];
  const narrow=(value:WorkflowRole["budgets"],ceiling:WorkflowRole["budgets"])=>({max_steps:Math.min(value.max_steps,ceiling.max_steps),max_seconds:Math.min(value.max_seconds,ceiling.max_seconds),max_output_tokens:Math.min(value.max_output_tokens,ceiling.max_output_tokens),max_total_tokens:Math.min(value.max_total_tokens??defaultRunBudgets.max_total_tokens,ceiling.max_total_tokens??defaultRunBudgets.max_total_tokens)});
  const role={...candidate,id:original.id,rights:{...(candidate.rights.current_workspace&&original.rights.current_workspace?{current_workspace:true}:{}),tools:candidate.rights.tools.filter(tool=>original.rights.tools.includes(tool)),folders:candidate.rights.folders.filter(folder=>original.rights.folders.includes(folder))},budgets:narrow(candidate.budgets,original.budgets),delegate_to:candidate.delegate_to.filter(id=>original.delegate_to.includes(id))};
  const priorAccess=original.agent_config?.access_mode,currentAccess=role.agent_config?.access_mode,accessModes=["read_only","confirm_all","confirm_mutations","autonomous"];
  if(priorAccess&&role.agent_config&&(!currentAccess||accessModes.indexOf(priorAccess)<accessModes.indexOf(currentAccess)))role.agent_config={...role.agent_config,access_mode:priorAccess};
  return {...refreshed,roles:refreshed.roles.map((entry,i)=>i===index?role:entry),steps:refreshed.steps.map(step=>step.role_id===original.id?{...step,rights:{...(step.rights.current_workspace&&role.rights.current_workspace?{current_workspace:true}:{}),tools:step.rights.tools.filter(tool=>role.rights.tools.includes(tool)),folders:step.rights.folders.filter(folder=>role.rights.folders.includes(folder))},budgets:narrow(step.budgets,role.budgets)}:step)};
}
