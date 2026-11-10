import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,defaultRunBudgets,type ProviderProfile,type WorkflowDefinition} from "@cowork/client";
import {LibraryError,LibraryMutationLatch,LibraryPreferencesLatch,libraryStorageKey,applyAgentToWorkflow,applyPersonalityToWorkflow,refreshWorkflowRoleFromLibrary,effectiveAgent,effectivePersonality,emptyAgentTemplate,emptyPersonality,exportLibraryItem,libraryIdentity,libraryPreferences,listLibrary,mcpLibraryAlias,mutateLibrary,parseLibraryImport,validateLibraryItem,writeLibraryPreferences,type AgentTemplate,type LibraryMutation,type Personality} from "./libraryApi";
import {validWorkflowEditor} from "./workflowUi";
import {definitiveRequestRejection} from "./requestJournal";

const profile:ProviderProfile={id:"profile-current",revision:4,name:"Current local model",kind:"ollama",model:"qwen",endpoint:"http://localhost:11434",credential_required:false,credential_ready:true,context_tokens:8192};
function personality():Personality{return {id:"personality",revision:2,created_at:1,updated_at:2,value:{...emptyPersonality(),name:"Reviewer",role:"reviewer",goal:"Find concrete defects",system_prompt:"Check the evidence.",skills_markdown:"Cite file locations.",generation:{temperature:0,timeout_ms:60000},is_default:true}};}
function template():AgentTemplate{return {id:"template",revision:3,created_at:1,updated_at:3,value:{...emptyAgentTemplate(),name:"Audit agent",personality:personality(),configuration:{...emptyAgentTemplate().configuration,instructions:"Review the requested change.",model:{kind:"provider",provider_id:profile.id,provider_revision:profile.revision},native_tools:["read_artifact","write_text_file"],allow_delegation:true,budgets:{...defaultRunBudgets,max_steps:1000}}}};}
function workflow():WorkflowDefinition{return {format_version:1,name:"Inspect",mode:{mode:"hierarchical",max_parallel:2,max_depth:2,max_delegations:2},failure:"continue_independent",rights:{tools:["read_artifact"],folders:[]},budgets:{...defaultRunBudgets,max_steps:5},roles:[{id:"Inspector",instructions:"Old",provider_id:"old",provider_revision:1,rights:{tools:["read_artifact"],folders:[]},budgets:{...defaultRunBudgets},delegate_to:["Reviewer","missing-role"]},{id:"Reviewer",instructions:"Review",provider_id:"old",provider_revision:1,rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],steps:[{id:"step",role_id:"Inspector",goal:"Inspect",dependencies:[],rights:{tools:["read_artifact"],folders:[]},budgets:{...defaultRunBudgets}}]};}
afterEach(()=>vi.unstubAllGlobals());
it("preserves explicit workspace selection when applying and refreshing an agent",async()=>{
  const value=workflow();value.rights.current_workspace=true;value.roles[0].rights.current_workspace=true;value.steps[0].rights.current_workspace=true;
  const agent=template(),applied=await applyAgentToWorkflow(value,0,agent,[profile]);
  expect(applied.roles[0].rights.current_workspace).toBe(true);expect(applied.steps[0].rights.current_workspace).toBe(true);
  const refreshed=await refreshWorkflowRoleFromLibrary(applied,0,{...agent,revision:agent.revision+1},[profile]);
  expect(refreshed.roles[0].rights.current_workspace).toBe(true);expect(refreshed.steps[0].rights.current_workspace).toBe(true);
});
describe("library transport and recovery",()=>{
  it.each([401,403,408,425,429])("retains a committed actor library save through lost ACK and HTTP %i until its exact receipt arrives",async(status)=>{
    const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const context={client:new Client("OWNER-NEVER","https://host"),session:"EXPIRED-ACTOR",actorId:"actor-one"},rotated={...context,session:"ROTATED-ACTOR"},key=libraryStorageKey(context,"personalities"),first=new LibraryMutationLatch(key,storage);
    const original=first.prepare(()=>({kind:"personalities",method:"POST",body:{idempotency_key:"already-saved",expected_revision:0,value:personality().value}}));
    const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost ACK after commit")).mockResolvedValueOnce(new Response(JSON.stringify({error:"retry-not-settled"}),{status})).mockResolvedValueOnce(new Response(JSON.stringify({...personality(),revision:1})));vi.stubGlobal("fetch",fetch);
    async function attempt(scope:typeof context,latch:LibraryMutationLatch){try{const saved=await mutateLibrary(scope,latch.prepare(()=>{throw new Error("never replace original");}));latch.confirm();return saved;}catch(error){if(error instanceof LibraryError&&definitiveRequestRejection(error.status))latch.confirm();throw error;}}
    await expect(attempt(context,first)).rejects.toThrow("lost ACK");const reloaded=new LibraryMutationLatch(key,storage);await expect(attempt(context,reloaded)).rejects.toMatchObject({status});expect(reloaded.pending).toBe(true);
    expect(libraryStorageKey(rotated,"personalities")).toBe(key);const recovered=new LibraryMutationLatch(key,storage);expect([...values.values()].join()).not.toMatch(/OWNER-NEVER|EXPIRED-ACTOR|ROTATED-ACTOR/);await attempt(rotated,recovered);expect(values.size).toBe(0);
    expect(fetch.mock.calls.map(call=>call[1].body)).toEqual([JSON.stringify(original.body),JSON.stringify(original.body),JSON.stringify(original.body)]);expect(fetch.mock.calls[2][1].headers.Authorization).toBe("Bearer ROTATED-ACTOR");expect(fetch.mock.calls.every(call=>call[0]==="https://host/api/v1/library/personalities")).toBe(true);
  });
  it("restores an exact lost-ACK request after reload without persisting bearer tokens",async()=>{
    const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const context={client:new Client("OWNER-BEARER","https://example.com"),session:"ACTOR-BEARER",actorId:"actor-one"},key=libraryStorageKey(context,"personalities"),first=new LibraryMutationLatch(key,storage);
    const original=first.prepare(()=>({kind:"personalities",method:"POST",body:{idempotency_key:"original-reload",expected_revision:0,value:personality().value}}));
    const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost response")).mockResolvedValueOnce(new Response(JSON.stringify({...personality(),revision:1})));vi.stubGlobal("fetch",fetch);
    await expect(mutateLibrary(context,original)).rejects.toThrow();
    expect([...values.entries()].flat().join(" ")).not.toMatch(/OWNER-BEARER|ACTOR-BEARER/);
    const reloaded=new LibraryMutationLatch(key,storage),build=vi.fn(()=>{throw new Error("must not rebuild");});expect(reloaded.pending).toBe(true);await mutateLibrary(context,reloaded.prepare(build));expect(build).not.toHaveBeenCalled();expect(fetch.mock.calls[1][1].body).toBe(fetch.mock.calls[0][1].body);
    reloaded.confirm();expect(storage.getItem(key)).toBeNull();
  });
  it("keeps malformed recovery state read-only until its local reference is explicitly discarded",()=>{
    const values=new Map<string,string>([["journal","{bad json"]]),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const latch=new LibraryMutationLatch("journal",storage);expect(latch.invalid).toBe(true);expect(latch.pending).toBe(true);expect(()=>latch.prepare(()=>{throw new Error("build");})).toThrow("request_journal_invalid");expect(storage.getItem("journal")).toBe("{bad json");latch.discard();expect(latch.pending).toBe(false);expect(storage.getItem("journal")).toBeNull();
  });
  it("persists default-agent preference CAS unchanged across reload and rejects credential-bearing journals",()=>{
    const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const first=new LibraryPreferencesLatch("preferences",storage),request={idempotency_key:"default-original",expected_revision:4,value:{default_agent_id:"template",system_prompt:"Use evidence"}};first.prepare(()=>request);const reloaded=new LibraryPreferencesLatch("preferences",storage);expect(reloaded.prepare(()=>{throw new Error();})).toEqual(request);reloaded.confirm();
    const unsafe=new LibraryPreferencesLatch("unsafe",storage);expect(()=>unsafe.prepare(()=>({...request,value:{default_agent_id:"template",api_key:"never-store"}}))).toThrow();expect(storage.getItem("unsafe")).toBeNull();
  });
  it("uses the explicit actor session and never retries owner auth on denial",async()=>{
    const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify({error:"forbidden"}),{status:403}));vi.stubGlobal("fetch",fetch);
    await expect(listLibrary({client:new Client("OWNER","https://host"),session:"ACTOR",actorId:"account-a"},"personalities")).rejects.toMatchObject({status:403});
    expect(fetch).toHaveBeenCalledTimes(1);expect(fetch.mock.calls[0][0]).toBe("https://host/api/v1/library/personalities");
    expect(fetch.mock.calls[0][1].headers.Authorization).toBe("Bearer ACTOR");expect(fetch.mock.calls[0][1].credentials).toBe("omit");
  });
  it("rejects incomplete actor context before requesting anything",()=>{
    expect(()=>libraryIdentity({client:new Client("OWNER"),actorId:"account-a"})).toThrow("library_actor_session_required");
    expect(()=>libraryIdentity({client:new Client("OWNER"),session:"ACTOR"})).toThrow("library_actor_session_required");
  });
  it("loads an owner list from the native versioned route and validates records",async()=>{
    const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify([personality()])));vi.stubGlobal("fetch",fetch);
    const rows=await listLibrary({client:new Client("OWNER","https://host")},"personalities");expect(rows[0]).toEqual(personality());
    expect(fetch.mock.calls[0][0]).toBe("https://host/api/v2/library/personalities");expect(fetch.mock.calls[0][1].headers.Authorization).toBe("Bearer OWNER");
    expect(()=>validateLibraryItem("personalities",{...personality(),revision:0})).toThrow();
    expect(()=>validateLibraryItem("personalities",{...personality(),value:{...personality().value,generation:{temperature:NaN}}})).toThrow();
  });
  it("retains identical request key and contents after a lost create acknowledgement",async()=>{
    const latch=new LibraryMutationLatch();
    const first=latch.prepare(()=>({kind:"personalities",method:"POST",body:{idempotency_key:"original",expected_revision:0,value:personality().value}}));
    const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost response")).mockResolvedValueOnce(new Response(JSON.stringify({...personality(),revision:1})));vi.stubGlobal("fetch",fetch);
    await expect(mutateLibrary({client:new Client("OWNER")},first)).rejects.toThrow("lost response");expect(latch.pending).toBe(true);
    const retry=latch.prepare(()=>({kind:"agent-templates",method:"POST",body:{idempotency_key:"new-key",expected_revision:0,value:template().value}}));
    await mutateLibrary({client:new Client("OWNER")},retry);expect(fetch.mock.calls[1][1].body).toBe(fetch.mock.calls[0][1].body);expect(retry.body.idempotency_key).toBe("original");
    latch.confirm();expect(latch.pending).toBe(false);
  });
  it("rejects a response with the wrong revision or entry instead of acknowledging it",async()=>{
    const mutation:LibraryMutation={kind:"personalities",id:"personality",method:"PUT",body:{idempotency_key:"key",expected_revision:2,value:personality().value}};
    vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify({...personality(),id:"other",revision:3}))));
    await expect(mutateLibrary({client:new Client("OWNER")},mutation)).rejects.toBeInstanceOf(LibraryError);
  });
  it("sets an actor's default agent by CAS while preserving all unrelated preferences",async()=>{
    const original={revision:6,updated_at:100,value:{default_agent_id:null,theme:"dark",system_prompt:"Keep evidence",future_setting:{retained:true}}};
    const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify(original))).mockImplementationOnce((_url,init)=>new Response(JSON.stringify({revision:7,updated_at:101,value:JSON.parse(init.body).value})));vi.stubGlobal("fetch",fetch);
    const context={client:new Client("OWNER","https://host"),session:"ACTOR",actorId:"actor"};
    const prefs=await libraryPreferences(context);await writeLibraryPreferences(context,{idempotency_key:"original-key",expected_revision:prefs.revision,value:{...prefs.value,default_agent_id:"template"}});
    expect(fetch.mock.calls.map(call=>call[0])).toEqual(["https://host/api/v1/preferences","https://host/api/v1/preferences"]);
    expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({idempotency_key:"original-key",expected_revision:6,value:{...original.value,default_agent_id:"template"}});
    expect(fetch.mock.calls[1][1].headers.Authorization).toBe("Bearer ACTOR");
  });
});
describe("portable content and application snapshots",()=>{
  it("refreshes the recorded source deliberately without widening current role limits",async()=>{
    const original=workflow(),item=template();original.rights.tools.push("write_text_file");original.roles[0].agent_config={...effectiveAgent(item),access_mode:"read_only"};original.roles[0].budgets.max_steps=2;
    item.revision=4;item.value.configuration.instructions="Updated source instructions";item.value.configuration.access_mode="autonomous";item.value.configuration.tool_filter=["read_artifact","write_text_file"];
    const before=structuredClone(original),next=await refreshWorkflowRoleFromLibrary(original,0,item,[profile]);
    expect(original).toEqual(before);expect(next.roles[0].id).toBe("Inspector");expect(next.roles[0].rights.tools).toEqual(["read_artifact"]);expect(next.roles[0].budgets.max_steps).toBe(2);expect(next.steps[0].budgets.max_steps).toBe(2);expect(next.roles[0].agent_config?.access_mode).toBe("read_only");expect(next.roles[0].agent_config?.agent_source).toEqual({id:"template",revision:4});expect(next.roles[0].instructions).toContain("Updated source instructions");expect(next.roles[1]).toEqual(before.roles[1]);expect(next.rights).toEqual(before.rights);
    await expect(refreshWorkflowRoleFromLibrary(original,0,{...item,id:"another"},[profile])).rejects.toThrow("library_source_changed");await expect(refreshWorkflowRoleFromLibrary(original,0,{...item,revision:1},[profile])).rejects.toThrow("library_source_changed");await expect(refreshWorkflowRoleFromLibrary(original,0,personality(),[profile])).rejects.toThrow("library_source_changed");
  });
  it("accepts saved personalities and agent snapshots longer than the former16k workflow limit without truncating",async()=>{
    const original=workflow(),p=personality();original.roles[0].delegate_to=["Reviewer"];p.value.system_prompt="P".repeat(20000);original.roles[0]=applyPersonalityToWorkflow(original.roles[0],p,[profile]);expect(original.roles[0].instructions).toContain(p.value.system_prompt);expect(validWorkflowEditor(original)).toBe(true);
    p.revision++;p.value.system_prompt="Updated long "+"P".repeat(20000);const refreshed=await refreshWorkflowRoleFromLibrary(original,0,p,[profile]);expect(refreshed.roles[0].agent_config?.personality_source?.revision).toBe(p.revision);expect(refreshed.roles[0].instructions).toContain(p.value.system_prompt);
    const agent=template();agent.value.configuration.instructions="A".repeat(20000);const applied=await applyAgentToWorkflow(workflow(),0,agent,[profile]);expect(applied.roles[0].instructions).toContain(agent.value.configuration.instructions);expect(validWorkflowEditor(applied)).toBe(true);
  });
  it("imports a copy without silently replacing the default personality",()=>{
    const original=personality().value,raw=exportLibraryItem("personalities",original),copy=parseLibraryImport(raw,"personalities");
    expect(copy).toEqual({...original,is_default:false});expect(original.is_default).toBe(true);
    expect(()=>parseLibraryImport(raw,"agent-templates")).toThrow("library_import_kind_mismatch");
    const imported=parseLibraryImport(exportLibraryItem("agent-templates",template().value),"agent-templates");expect("configuration" in imported).toBe(true);if("configuration" in imported){expect(imported.personality).toBeNull();expect(imported.configuration.instructions).toContain("Check the evidence.");expect(imported.configuration.instructions).toContain("Review the requested change.");expect(imported.configuration.generation.temperature).toBe(0);expect(imported.configuration.personality_source).toBeUndefined();}
  });
  it("copies instructions, generation values and immutable source revisions",()=>{
    const p=personality(),item=template(),effective=effectiveAgent(item);
    expect(effective.instructions).toContain("Check the evidence.");expect(effective.instructions).toContain("Cite file locations.");expect(effective.instructions).toContain("Review the requested change.");
    expect(effective.personality_source).toEqual({id:"personality",revision:2});expect(effective.agent_source).toEqual({id:"template",revision:3});expect(effective.generation.temperature).toBe(0);
    const copy=effectivePersonality(p);p.value.generation.temperature=1;expect(copy.generation.temperature).toBe(0);
    const role=applyPersonalityToWorkflow(workflow().roles[0],personality());expect(role.instructions).toContain("Role: reviewer");expect(role.provider_id).toBe("old");
  });
  it("freezes dedicated backstory and tool restrictions without widening workflow rights",async()=>{
    const item=template();item.value.configuration.backstory="Experienced source auditor";item.value.configuration.tool_filter=["read_artifact","write_text_file"];item.value.configuration.skill_ids=["skill-one"];item.value.configuration.memory_ids=["memory-one"];
    const effective=effectiveAgent(item);expect(effective.instructions).toContain("Backstory: Experienced source auditor");expect(effective.tool_filter).toEqual(["read_artifact","write_text_file"]);expect(effective.access_mode).toBe(item.value.configuration.access_mode);
    item.value.configuration.tool_filter.push("execute_program");expect(effective.tool_filter).not.toContain("execute_program");
    expect(effective.skill_ids).toEqual(["skill-one"]);expect(effective.memory_ids).toEqual(["memory-one"]);item.value.configuration.memory_ids.push("later");expect(effective.memory_ids).toEqual(["memory-one"]);
    const result=await applyAgentToWorkflow(workflow(),0,item,[profile]);expect(result.roles[0].rights.tools).toEqual(["read_artifact"]);expect(result.roles[0].agent_config?.tool_filter).toContain("write_text_file");
  });
  it("applies templates within the existing rights and budgets, preserves role bindings and removes invalid delegates",async()=>{
    const source=workflow(),before=structuredClone(source),item=template(),result=await applyAgentToWorkflow(source,0,item,[profile]);
    expect(source).toEqual(before);expect(result.roles[0].id).toBe("Inspector");expect(result.steps[0].role_id).toBe("Inspector");
    expect(result.roles[0].provider_id).toBe(profile.id);expect(result.roles[0].provider_revision).toBe(4);
    expect(result.roles[0].rights.tools).toEqual(["read_artifact"]);expect(result.roles[0].budgets.max_steps).toBe(5);expect(result.roles[0].delegate_to).toEqual(["Reviewer"]);
    expect(result.roles[1]).toEqual(before.roles[1]);expect(result.rights).toEqual(before.rights);
    expect(result.roles[0].agent_config?.agent_source).toEqual({id:"template",revision:3});
  });
  it("refuses stale providers and unsupported Codex workflow bindings",async()=>{
    await expect(applyAgentToWorkflow(workflow(),0,template(),[{...profile,revision:5}])).rejects.toThrow("library_provider_changed");
    const item=template();item.value.configuration.model={kind:"codex",account_id:"account",model:null,reasoning_effort:null};
    await expect(applyAgentToWorkflow(workflow(),0,item,[profile])).rejects.toThrow("library_workflow_codex_unavailable");
  });
  it("uses canonical MCP aliases and intersects them with explicitly granted workflow aliases",async()=>{
    const alias=await mcpLibraryAlias("server-1","search");expect(alias).toMatch(/^mcp_[0-9a-f]{40}$/);
    const source=workflow();source.rights.tools=[alias];const item=template();item.value.configuration.mcp_tools=[{server_id:"server-1",server_revision:2,tool_name:"search"}];
    const result=await applyAgentToWorkflow(source,0,item,[profile]);expect(result.roles[0].rights.tools).toEqual([alias]);
    source.rights.tools=[];expect((await applyAgentToWorkflow(source,0,item,[profile])).roles[0].rights.tools).toEqual([]);
  });
});
