import {useEffect,useRef,useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {defaultRunBudgets,type AccessMode,type Budgets,type Client,type ProviderProfile} from "@cowork/client";
import {AgentLibrarySelector,useAgentLibrary} from "./AgentLibrary";
import {materializeCodexModelProfile} from "./codexModelApi";
import {RunBudgets} from "./RunBudgets";
import {de,en} from "./i18n";
import {useView} from "./view";
import {effectiveAgent,effectivePersonality,mcpLibraryAlias,libraryIdentity,libraryPreferences,writeLibraryPreferences,resolveAgentModel,LibraryPreferencesLatch,libraryStorageKey,type AgentTemplate,type Personality,type EffectiveAgentConfig,type LibraryPreferences,type AgentModelSelection} from "./libraryApi";

export type ActorModelProfile={id:string;revision:number;name:string;model_defaults:{model:string;context_tokens?:number;auth_mode:string}};
export type ActorRunConfiguration={agent_config:EffectiveAgentConfig;access_mode:AccessMode;budgets:Required<Budgets>};
export type ActorAgentSelection={run_configuration:ActorRunConfiguration;skill_ids:string[];memory_ids:string[];mcp_metadata_ids:string[];expected_mcp_revisions:Record<string,number>};
export function validateActorRunConfiguration(value:unknown):ActorRunConfiguration{
  const raw=value as ActorRunConfiguration;
  if(!raw||typeof raw!=="object"||Object.keys(raw).some(k=>!["agent_config","access_mode","budgets"].includes(k))||!raw.agent_config||typeof raw.agent_config.instructions!=="string"||new TextEncoder().encode(raw.agent_config.instructions).length>256000||raw.agent_config.instructions.includes("\0")||!raw.agent_config.generation||Object.keys(raw.agent_config).some(k=>!["instructions","generation","tool_filter","access_mode","skill_ids","memory_ids","personality_source","agent_source","execution_mode","behavior"].includes(k))||Object.keys(raw.agent_config.generation).some(k=>!["temperature","timeout_ms"].includes(k))||!["read_only","confirm_mutations","confirm_all","autonomous"].includes(raw.access_mode)||raw.agent_config.access_mode!==undefined&&!["read_only","confirm_mutations","confirm_all","autonomous"].includes(raw.agent_config.access_mode)||!raw.budgets)throw new Error("private_agent_configuration_invalid");
  for(const [key,min,max] of [["max_steps",1,4096],["max_seconds",1,86400],["max_output_tokens",1,1000000],["max_total_tokens",1024,1000000000]] as const){const n=raw.budgets[key];if(!Number.isSafeInteger(n)||n<min||n>max)throw new Error("private_agent_configuration_invalid");}
  const g=raw.agent_config.generation;if(g.temperature!==undefined&&(!Number.isFinite(g.temperature)||g.temperature<0||g.temperature>2)||g.timeout_ms!==undefined&&(!Number.isSafeInteger(g.timeout_ms)||g.timeout_ms<1000||g.timeout_ms>1200000))throw new Error("private_agent_configuration_invalid");
  const tools=raw.agent_config.tool_filter;if(tools!==undefined&&(!Array.isArray(tools)||tools.length>128||tools.some(s=>typeof s!=="string"||!s||s.length>200)||new Set(tools).size!==tools.length))throw new Error("private_agent_configuration_invalid");
  for(const ids of [raw.agent_config.skill_ids,raw.agent_config.memory_ids])if(ids!==undefined&&(!Array.isArray(ids)||ids.length>128||ids.some(id=>typeof id!=="string"||!id||id.length>128)||new Set(ids).size!==ids.length))throw new Error("private_agent_configuration_invalid");
  if(raw.agent_config.execution_mode!==undefined&&!["execute","plan","hybrid"].includes(raw.agent_config.execution_mode))throw new Error("private_agent_configuration_invalid");
  const b=raw.agent_config.behavior;if(b!==undefined&&(typeof b.max_tool_calls_per_loop!=="number"||!Number.isSafeInteger(b.max_tool_calls_per_loop)||b.max_tool_calls_per_loop<1||b.max_tool_calls_per_loop>4096||typeof b.fallback_to_human_on_repeated_failure!=="boolean"||!Array.isArray(b.command_allow_groups)||b.command_allow_groups.length>16||b.command_allow_groups.some(g=>!Array.isArray(g)||!g.length||g.length>256||g.some(p=>typeof p!=="string"||!p||p.length>4096||p.includes("\0")))||!Array.isArray(b.command_blacklist)||b.command_blacklist.length>256||b.command_blacklist.some(p=>typeof p!=="string"||!p||p.length>4096||p.includes("\0"))||Object.keys(b).some(k=>!["command_allow_groups","command_blacklist","max_tool_calls_per_loop","fallback_to_human_on_repeated_failure"].includes(k))))throw new Error("private_agent_configuration_invalid");
  return structuredClone(raw);
}
export async function copiedActorSelection(preferences:LibraryPreferences,agent:AgentTemplate|null,personality:Personality|null,providers:ActorModelProfile[]):Promise<{selection:ActorAgentSelection;model:AgentModelSelection}>{
  const defaults=preferences.value;
  const model:AgentModelSelection=agent?resolveAgentModel({...agent,value:{...agent.value,personality}}):personality?.value.model??{kind:"inherit"};
  if(model.kind==="provider"&&!providers.some(p=>p.id===model.provider_id&&p.revision===model.provider_revision))throw new Error("private_agent_provider_changed");
  const config:EffectiveAgentConfig=agent?effectiveAgent({...agent,value:{...agent.value,personality}}):personality?effectivePersonality(personality):{instructions:"",generation:{}};
  config.instructions=[typeof defaults.system_prompt==="string"?defaults.system_prompt:"",config.instructions].filter(Boolean).join("\n\n");
  const c=agent?.value.configuration;
  const revisions=new Map<string,number>();for(const tool of c?.mcp_tools??[]){if(revisions.has(tool.server_id)&&revisions.get(tool.server_id)!==tool.server_revision)throw new Error("private_agent_mcp_revision_conflict");revisions.set(tool.server_id,tool.server_revision);}
  if(c&&c.tool_filter===undefined){const mcp=await Promise.all(c.mcp_tools.map(tool=>mcpLibraryAlias(tool.server_id,tool.tool_name)));config.tool_filter=[...new Set(["list_files","read_text_file","stat_file","write_text_file","create_directory","rename_file","remove_file","restore_file","create_artifact","list_artifacts","read_artifact","request_input",...c.native_tools,...mcp])];}
  const selection:ActorAgentSelection={run_configuration:{agent_config:config,access_mode:c?.access_mode??(defaults.default_access_mode as AccessMode)??"confirm_mutations",budgets:{...defaultRunBudgets,...(defaults.default_budgets as Budgets|undefined),...c?.budgets}},skill_ids:[...c?.skill_ids??[]],memory_ids:[...c?.memory_ids??[]],mcp_metadata_ids:[...new Set(c?.mcp_tools.map(t=>t.server_id)??[])],expected_mcp_revisions:Object.fromEntries(c?.mcp_tools.map(tool=>[tool.server_id,tool.server_revision])??[])};
  validateActorRunConfiguration(selection.run_configuration);
  if(selection.mcp_metadata_ids.length>16||selection.skill_ids.length>100||selection.memory_ids.length>100||[...selection.skill_ids,...selection.memory_ids,...selection.mcp_metadata_ids].some(id=>!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(id)))throw new Error("private_agent_context_binding_invalid");
  return {selection,model};
}

export function ActorAgentConfigurationPanel({client,session,actorId,providers,profilesReady,selectedProfile,initialSelection,disabled,onProfile,onSelection,onMcpSelection,onCodexProfile}:{client:Client;session:string;actorId:string;providers:ActorModelProfile[];profilesReady:boolean;selectedProfile:string;initialSelection?:ActorAgentSelection;disabled:boolean;onProfile:(id:string)=>void;onSelection:(selection:ActorAgentSelection|undefined)=>void;onMcpSelection:(ids:string[])=>void;onCodexProfile?:(profile:ProviderProfile)=>void}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b,t=(key:keyof typeof de)=>(language==="de"?de:en)[key];
  const context={client,session,actorId},identity=libraryIdentity(context),qc=useQueryClient(),library=useAgentLibrary(client,session,actorId);
  const defaults=useQuery({queryKey:["agent-library-defaults",identity],queryFn:()=>libraryPreferences(context),retry:false});
  const [agent,setAgent]=useState<AgentTemplate|null>(null),[personality,setPersonality]=useState<Personality|null>(null),[selection,setSelection]=useState<ActorAgentSelection>(),[error,setError]=useState(""),[defaultsDraft,setDefaultsDraft]=useState<LibraryPreferences>(),[busy,setBusy]=useState(false);
  const initialized=useRef(false),epoch=useRef(0),latch=useRef(new LibraryPreferencesLatch(libraryStorageKey(context,"actor-defaults")));
  useEffect(()=>{epoch.current++;initialized.current=false;setAgent(null);setPersonality(null);setSelection(undefined);setDefaultsDraft(undefined);setError("");onSelection(undefined);return()=>{epoch.current++;};},[identity]);
  async function install(a:AgentTemplate|null,p:Personality|null,preferences=defaults.data){
    if(!preferences||disabled||busy)return;const generation=epoch.current;setBusy(true);setSelection(undefined);onSelection(undefined);
    try{const copied=await copiedActorSelection(preferences,a,p,providers);
      const materialized=copied.model.kind==="codex"?await materializeCodexModelProfile(context,copied.model):undefined;
      if(generation!==epoch.current)return;
      setAgent(a?structuredClone(a):null);setPersonality(p?structuredClone(p):null);setSelection(copied.selection);onSelection(structuredClone(copied.selection));onMcpSelection([...copied.selection.mcp_metadata_ids]);
      if(copied.model.kind==="provider")onProfile(copied.model.provider_id);else if(materialized){onCodexProfile?.(materialized);onProfile(materialized.id);}setError("");
    }catch(e){if(generation===epoch.current)setError(e instanceof Error?e.message:String(e));}finally{if(generation===epoch.current)setBusy(false);}
  }

  useEffect(()=>{
    if(initialized.current||disabled||!profilesReady||!defaults.data||!library.agents.data||!library.personalities.data)return;
    if(initialSelection){initialized.current=true;setSelection(structuredClone(initialSelection));onSelection(structuredClone(initialSelection));onMcpSelection([...initialSelection.mcp_metadata_ids]);setDefaultsDraft(structuredClone(defaults.data));return;}
    const pref=defaults.data.value;
    setDefaultsDraft(structuredClone(defaults.data));
    const a=typeof pref.default_agent_id==="string"?library.agents.data.find(a=>a.id===pref.default_agent_id)??null:null;
    const p=a?.value.personality??(typeof pref.default_personality_id==="string"?library.personalities.data.find(p=>p.id===pref.default_personality_id)??null:library.personalities.data.find(p=>p.value.is_default)??null);
    initialized.current=true;
    const providerDefaults=pref.provider_defaults&&typeof pref.provider_defaults==="object"?pref.provider_defaults as Record<string,unknown>:{};
    const defaultProvider=pref.default_provider_id??providerDefaults.openai_compatible;
    if(defaultProvider&&!providers.some(p=>p.id===defaultProvider)){setError("private_default_provider_missing");return;}
    if(pref.default_agent_id&&!a||pref.default_personality_id&&!p){setError("private_default_library_entry_missing");return;}
    if(typeof defaultProvider==="string")onProfile(defaultProvider);
    void install(a,p,defaults.data);
  },[defaults.data,library.agents.data,library.personalities.data,providers,profilesReady,disabled]);
  function edit(patch:Partial<ActorRunConfiguration>){if(!selection||disabled)return;const next={...selection,run_configuration:{...selection.run_configuration,...patch}};setSelection(next);onSelection(structuredClone(next));}
  async function saveDefaults(event:FormEvent){event.preventDefault();if(!defaultsDraft)return;setBusy(true);setError("");const generation=epoch.current;try{
    const request=latch.current.prepare(()=>({idempotency_key:crypto.randomUUID(),expected_revision:defaultsDraft.revision,value:structuredClone(defaultsDraft.value)}));
    const saved=await writeLibraryPreferences(context,request);if(generation!==epoch.current)return;latch.current.confirm();setDefaultsDraft(saved);qc.setQueryData(["agent-library-defaults",identity],saved);
  }catch(e){if(generation===epoch.current)setError(e instanceof Error?e.message:String(e));}finally{if(generation===epoch.current)setBusy(false);}}
  async function reloadDefaults(){setBusy(true);const generation=epoch.current;try{const saved=await libraryPreferences(context);if(generation!==epoch.current)return;latch.current.discard();setDefaultsDraft(saved);qc.setQueryData(["agent-library-defaults",identity],saved);setError("");}catch(e){if(generation===epoch.current)setError(e instanceof Error?e.message:String(e));}finally{if(generation===epoch.current)setBusy(false);}}
  const profile=providers.find(p=>p.id===selectedProfile),isLoading=defaults.isPending||library.agents.isPending||library.personalities.isPending;
  return <section className="product-item"><h3>{tr("Agent und Persönlichkeit","Agent and personality")}</h3>
    <AgentLibrarySelector client={client} session={session} actorId={actorId} disabled={disabled||isLoading||busy} agentId={agent?.id} personalityId={personality?.id} onAgent={a=>void install(a,a?.value.personality??null)} onPersonality={p=>void install(agent,p)}/>
    <button type="button" disabled={disabled||isLoading||busy} onClick={()=>void defaults.refetch().then(result=>install(agent,personality,result.data))}>{tr("Auswahl erneut übernehmen","Apply selection again")}</button>
    {error&&<p role="alert" className="error">{error}</p>}{(defaults.isError||library.agents.isError||library.personalities.isError)&&<p role="alert">{tr("Die persönlichen Agentstandards konnten nicht geladen werden.","Personal agent defaults could not be loaded.")}<button type="button" onClick={()=>{void defaults.refetch();void library.agents.refetch();void library.personalities.refetch();}}>{tr("Erneut laden","Retry")}</button></p>}
    {selection&&<fieldset disabled={disabled||busy}><p>{tr("Die Auswahl wird als unabhängige Kopie für diesen neuen Lauf übernommen.","Selection is copied independently into this new run.")}</p><label>{tr("Anweisungen für diesen Lauf","Instructions for this run")}<textarea rows={5} maxLength={256000} value={selection.run_configuration.agent_config.instructions} onChange={e=>edit({agent_config:{...selection.run_configuration.agent_config,instructions:e.target.value}})}/></label>
      <label>{tr("Aktionsfreigaben","Approval mode")}<select value={selection.run_configuration.access_mode} onChange={e=>edit({access_mode:e.target.value as AccessMode})}>{(["read_only","confirm_mutations","confirm_all","autonomous"] as const).map(mode=><option value={mode} key={mode}>{t(mode)}</option>)}</select></label>
      <label>{tr("Temperatur","Temperature")}<input type="number" min={0} max={2} step={0.05} value={selection.run_configuration.agent_config.generation.temperature??""} onChange={e=>edit({agent_config:{...selection.run_configuration.agent_config,generation:{...selection.run_configuration.agent_config.generation,temperature:e.target.value===""?undefined:Number(e.target.value)}}})}/></label>
      <label>{tr("Zeitlimit je Modellanfrage (ms)","Model request timeout (ms)")}<input type="number" min={1000} max={1200000} step={1000} value={selection.run_configuration.agent_config.generation.timeout_ms??""} onChange={e=>edit({agent_config:{...selection.run_configuration.agent_config,generation:{...selection.run_configuration.agent_config.generation,timeout_ms:e.target.value===""?undefined:Number(e.target.value)}}})}/></label>
      <RunBudgets value={selection.run_configuration.budgets} setValue={budgets=>edit({budgets})} disabled={disabled} t={t} contextTokens={profile?.model_defaults.context_tokens}/>
    </fieldset>}
    {!disabled&&defaultsDraft&&<details><summary>{tr("Standards dieses Serverkontos bearbeiten","Edit this server account's defaults")}</summary><form onSubmit={e=>void saveDefaults(e)}><fieldset disabled={busy||latch.current.pending}><label>{tr("Standardmodellprofil","Default model profile")}<select value={String(defaultsDraft.value.default_provider_id??"")} onChange={e=>setDefaultsDraft({...defaultsDraft,value:{...defaultsDraft.value,default_provider_id:e.target.value||null}})}><option value="">{tr("Je Lauf auswählen","Choose per run")}</option>{providers.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label><label>{tr("Standardagent","Default agent")}<select value={String(defaultsDraft.value.default_agent_id??"")} onChange={e=>setDefaultsDraft({...defaultsDraft,value:{...defaultsDraft.value,default_agent_id:e.target.value||null}})}><option value="">{tr("Keine Vorlage","No template")}</option>{library.agents.data?.map(a=><option key={a.id} value={a.id}>{a.value.name}</option>)}</select></label><label>{tr("Zusätzliche Anweisungen","Additional instructions")}<textarea maxLength={64000} rows={4} value={String(defaultsDraft.value.system_prompt??"")} onChange={e=>setDefaultsDraft({...defaultsDraft,value:{...defaultsDraft.value,system_prompt:e.target.value}})}/></label></fieldset><button disabled={busy}>{tr(latch.current.pending?"Ursprüngliche Speicherung abgleichen":"Kontostandards speichern",latch.current.pending?"Reconcile original save":"Save account defaults")}</button>{latch.current.pending&&<button type="button" disabled={busy} onClick={()=>void reloadDefaults()}>{tr("Aktuelle Standards laden und lokalen Entwurf verwerfen","Load current defaults and discard local draft")}</button>}</form></details>}
  </section>;
}
