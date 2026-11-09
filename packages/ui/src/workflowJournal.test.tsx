import {afterEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,defaultRunBudgets,type WorkflowDefinition,type WorkflowDraftContent,type WorkflowOperation,type PrepareWorkflowOperation} from "@cowork/client";
import {WorkflowOperationSession,WorkflowRecoveryRequired,loadWorkflowRecovery,restoreWorkflowEditor,workflowEditorFormat,WorkflowOperationPanel,workflowDraftKey} from "./workflowJournal";
import {WorkflowEditor} from "./WorkflowLibrary";
import {en} from "./i18n";
import {renameWorkflowRole,validWorkflowEditor} from "./workflowUi";
const definition=():WorkflowDefinition=>({format_version:1,name:"Original",mode:{mode:"sequential"},failure:"continue_independent",rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},roles:[{id:"Agent",instructions:"Review",provider_id:"local",provider_revision:2,rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],steps:[{id:"step",role_id:"Agent",goal:"Review",dependencies:[],rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets}}]});
const content=():WorkflowDraftContent=>({editor_format:workflowEditorFormat,data:definition(),origin:null,archived:false});
const operation=():WorkflowOperation=>({id:"saved-operation",project_id:"project",intent_id:"intent",request_sha256:"a".repeat(64),request:{kind:"create",request:{idempotency_key:"original-key",definition:definition()}},draft_snapshot:null,state:"pending",receipt:null,rejection:null,created_at:1,updated_at:1,acknowledged_at:null});
const accepted=(value:WorkflowOperation):WorkflowOperation=>({...value,state:"accepted",receipt:{kind:"created",publication:{workflow_id:"workflow",project_id:value.project_id,revision:1,revoked:false,version_id:"first-version",created_at:2}},updated_at:2});
const json=(value:unknown,status=200)=>new Response(JSON.stringify(value),{status,headers:{"Content-Type":"application/json"}});
const matcher=(value:WorkflowOperation)=>value.request.kind!=="start";
afterEach(()=>vi.unstubAllGlobals());
describe("durable workflow UI recovery",()=>{
  it("reads all draft and unresolved-operation pages before any new command",async()=>{
    const calls:string[]=[];
    vi.stubGlobal("fetch",vi.fn(async(input:unknown)=>{const url=String(input);calls.push(url);
      return json(url.includes("workflow-drafts")?{items:[],total:0,next_offset:null}:url.includes("offset=0")?{items:[],total:1,next_offset:20}:{items:[operation()],total:1,next_offset:null});}));
    const client=new Client("fixture","http://fixture");const recovered=await loadWorkflowRecovery(client,"project");
    expect(recovered.operations).toHaveLength(1);expect(calls).toHaveLength(3);
    const session=new WorkflowOperationSession(client,"project",matcher);const build=vi.fn(()=>operation().request);
    await expect(session.prepare(build)).rejects.toBeInstanceOf(WorkflowRecoveryRequired);
    expect(build).not.toHaveBeenCalled();expect(session.operation?.id).toBe("saved-operation");
  });
  it("serializes simultaneous prepares before invoking the key factory",async()=>{
    let release!:(value:Response)=>void;let posts=0;
    vi.stubGlobal("fetch",vi.fn(async(_input:unknown,init?:RequestInit)=>{
      if(init?.method==="POST"){posts++;const body=decode("PrepareWorkflowOperation",JSON.parse(String(init.body)));return json({...operation(),intent_id:body.intent_id,request:body.request});}
      return new Promise<Response>(resolve=>{release=resolve;});
    }));
    const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);
    const build=vi.fn(()=>operation().request);const first=session.prepare(build);const second=session.prepare(build);
    expect(second).toBe(first);expect(build).not.toHaveBeenCalled();
    release(json({items:[],total:0,next_offset:null}));
    const results=await Promise.all([first,second]);expect(results[0].id).toBe(results[1].id);expect(build).toHaveBeenCalledTimes(1);expect(posts).toBe(1);
  });
  it("rejects a nonadvancing recovery page without minting a request",async()=>{
    vi.stubGlobal("fetch",vi.fn(async()=>json({items:[],total:1,next_offset:0})));
    const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);const build=vi.fn(()=>operation().request);
    await expect(session.prepare(build)).rejects.toThrow("workflowJournalUnavailable");expect(build).not.toHaveBeenCalled();expect(session.uncertainPrepare).toBe(false);
  });
  it("restores the same prepared request after lost acknowledgement without another key or publication",async()=>{
    const draft=definition();let saved:WorkflowOperation|null=null;let posts=0;const received:PrepareWorkflowOperation[]=[];
    vi.stubGlobal("fetch",vi.fn(async(_input:unknown,init?:RequestInit)=>{
      if(init?.method==="POST"){posts++;const body=decode("PrepareWorkflowOperation",JSON.parse(String(init.body)));received.push(body);
        saved={...operation(),intent_id:body.intent_id,request:body.request,draft_snapshot:body.draft_snapshot??null};throw new TypeError("lost prepare acknowledgement");}
      return json({items:saved?[saved]:[],total:saved?1:0,next_offset:null});
    }));
    const client=new Client("fixture","http://fixture");const session=new WorkflowOperationSession(client,"project",matcher);
    const build=vi.fn(()=>({kind:"create" as const,request:{idempotency_key:"one-key",definition:draft}}));
    await expect(session.prepare(build)).rejects.toThrow("lost prepare");
    draft.name="Later unsent edit";const confirmed=await session.prepare(build);
    expect(posts).toBe(1);expect(build).toHaveBeenCalledTimes(1);
    expect(confirmed.request).toEqual(received[0].request);
    const reopened=new WorkflowOperationSession(client,"project",matcher);
    const newKey=vi.fn(()=>operation().request);
    await expect(reopened.prepare(newKey)).rejects.toBeInstanceOf(WorkflowRecoveryRequired);
    expect(newKey).not.toHaveBeenCalled();expect(reopened.operation?.intent_id).toBe(confirmed.intent_id);
  });
  it("reconciles an accepted commit after a lost response and keeps the first immutable receipt",async()=>{
    let saved=operation();let commits=0;
    vi.stubGlobal("fetch",vi.fn(async(input:unknown,init?:RequestInit)=>{
      if(String(input).endsWith("/commit")){commits++;expect(init?.body).toBeUndefined();saved=accepted(saved);throw new TypeError("lost commit acknowledgement");}
      return json(saved);
    }));
    const client=new Client("fixture","http://fixture");const session=new WorkflowOperationSession(client,"project",matcher);session.attach(saved);
    await expect(session.commit()).rejects.toThrow("lost commit");
    expect(session.operation?.state).toBe("accepted");
    expect(session.operation?.receipt).toEqual(accepted(operation()).receipt);expect(commits).toBe(1);
    const markup=renderToStaticMarkup(<WorkflowOperationPanel operation={session.operation} busy={false} error={null} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).toContain("Open confirmed result");expect(markup).not.toContain(">Discard operation<");
  });
  it("retains Pending after provider failure and refuses a pending acknowledgement",async()=>{
    const saved=operation();const fetch=vi.fn(async(input:unknown)=>String(input).endsWith("/commit")?json({code:"provider_unavailable",message:"Provider offline",retryable:true,correlation_id:"fixture"},503):json(saved));
    vi.stubGlobal("fetch",fetch);const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);session.attach(saved);
    await expect(session.commit()).rejects.toThrow("Provider offline");expect(session.operation?.state).toBe("pending");
    const before=fetch.mock.calls.length;await expect(session.acknowledge()).rejects.toThrow("workflowJournalUnavailable");expect(fetch.mock.calls).toHaveLength(before);
  });
  it("shows Accepted when discard loses the commit race, without canceling the saved run or effect",async()=>{
    const paths:string[]=[];vi.stubGlobal("fetch",vi.fn(async(input:unknown,init?:RequestInit)=>{paths.push(String(input));expect(init?.body).toBeUndefined();return json(accepted(operation()));}));
    const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);session.attach(operation());
    expect((await session.discard()).state).toBe("accepted");expect(paths).toHaveLength(1);expect(paths[0]).toMatch(/\/discard$/);
  });
  it("requires terminal acknowledgement before a fresh operation and keeps contexts separate",async()=>{
    const records=new Map<string,WorkflowOperation>([["project",accepted(operation())]]);
    vi.stubGlobal("fetch",vi.fn(async(input:unknown,init?:RequestInit)=>{
      const url=String(input);const project=url.includes("/projects/other/")?"other":"project";const current=records.get(project);
      if(url.endsWith("/acknowledge"))return json({...current,acknowledged_at:4});
      if(init?.method==="POST"){const body=decode("PrepareWorkflowOperation",JSON.parse(String(init.body)));return json({...operation(),project_id:project,intent_id:body.intent_id,request:body.request});}
      return json({items:current?[current]:[],total:current?1:0,next_offset:null});
    }));
    const client=new Client("fixture","http://fixture");const first=new WorkflowOperationSession(client,"project",matcher);const build=vi.fn(()=>operation().request);
    await expect(first.prepare(build)).rejects.toBeInstanceOf(WorkflowRecoveryRequired);expect(build).not.toHaveBeenCalled();
    await first.acknowledge();expect(first.operation).toBeNull();
    const other=new WorkflowOperationSession(client,"other",matcher);expect((await other.prepare(build)).project_id).toBe("other");expect(build).toHaveBeenCalledTimes(1);
  });
  it("recognizes confirmed draft content after Rust sorts nested object keys without spending another snapshot",()=>{
    const input=content();
    const reorder=(item:unknown):unknown=>Array.isArray(item)?item.map(reorder):item&&typeof item==="object"?Object.fromEntries(Object.entries(item).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([key,value])=>[key,reorder(value)])):item;
    const server=reorder(input);expect(JSON.stringify(server)).not.toEqual(JSON.stringify(input));
    const saved=workflowDraftKey(server);expect(workflowDraftKey(input)).toBe(saved);
    const definition=structuredClone(input.data) as WorkflowDefinition;definition.steps.push({...definition.steps[0],id:"different"});
    expect(workflowDraftKey({...input,data:definition})).not.toBe(saved);
    const original=definition.steps.map(step=>({...step}));definition.steps.reverse();
    expect(workflowDraftKey(original)).not.toBe(workflowDraftKey(definition.steps));
    expect(workflowDraftKey({value:-2.5})).not.toBe(workflowDraftKey({value:0}));
  });
  it("preserves incomplete and temporarily invalid numeric form data without converting it to published rights",()=>{
    const value=definition();value.name="";value.roles[0].provider_id="";value.roles[0].provider_revision=0;value.budgets.max_steps=-2.5;
    const draft={...content(),data:value};const copy=structuredClone(draft);
    expect(restoreWorkflowEditor(draft)).toEqual(value);expect(draft).toEqual(copy);
    expect(()=>decode("WorkflowDefinition",value)).toThrow();
  });
  it("retains unknown or malformed editor formats read-only",()=>{
    const unknown={...content(),editor_format:"future-format",data:{unrecognized:"retained"}};const copy=structuredClone(unknown);
    expect(restoreWorkflowEditor(unknown)).toBeNull();expect(unknown).toEqual(copy);
    expect(restoreWorkflowEditor({...content(),data:{format_version:1,roles:"broken"}})).toBeNull();
  });
  it("renders a roleless saved draft without allowing Add step to dereference a missing role",()=>{
    const value=definition();value.roles=[];value.steps=[];const restored=restoreWorkflowEditor({...content(),data:value});expect(restored).not.toBeNull();
    const markup=renderToStaticMarkup(<WorkflowEditor value={restored!} setValue={()=>{}} providers={[]} disabled={false} t={key=>en[key]} onSubmit={()=>{}}/>);
    expect(markup).toMatch(/<button[^>]*disabled=""[^>]*>[\s\S]*?Add step/);
  });
  it("rejects A to B collisions and keeps each role's model binding through either later rename",()=>{
    const value=definition();value.roles=[{...value.roles[0],id:"A"},{...value.roles[0],id:"B",provider_id:"different-model"}];
    value.steps=[{...value.steps[0],id:"a",role_id:"A"},{...value.steps[0],id:"b",role_id:"B"}];
    const rejected=renameWorkflowRole(value,0,"B");expect(rejected.collision).toBe(true);expect(rejected.definition).toEqual(value);
    const first=renameWorkflowRole(rejected.definition,0,"C").definition;
    expect(first.roles.map(role=>role.id)).toEqual(["C","B"]);expect(first.steps.map(step=>step.role_id)).toEqual(["C","B"]);expect(validWorkflowEditor(first)).toBe(true);
    const second=renameWorkflowRole(rejected.definition,1,"C").definition;
    expect(second.steps.map(step=>step.role_id)).toEqual(["A","C"]);expect(validWorkflowEditor(second)).toBe(true);
  });
  it.each([0,1])("requires explicit reassignments when repairing an already saved duplicate on row %s",index=>{
    const value=definition();value.roles=[{...value.roles[0],id:"B"},{...value.roles[0],id:"B",provider_id:"different-model"}];
    value.steps=[{...value.steps[0],id:"a",role_id:"B"},{...value.steps[0],id:"b",role_id:"B"}];
    const repaired=renameWorkflowRole(value,index,"C").definition;
    expect(repaired.steps.map(step=>step.role_id)).toEqual(["",""]);expect(validWorkflowEditor(repaired)).toBe(false);
    const markup=renderToStaticMarkup(<WorkflowEditor value={repaired} setValue={()=>{}} providers={[]} disabled={false} t={key=>en[key]} onSubmit={()=>{}}/>);
    expect(markup).toContain(en.workflowRoleBindingRequired);expect(markup).toContain(en.workflowRoleBindingChoose);
    const explicit={...repaired,steps:repaired.steps.map((step,i)=>({...step,role_id:i===index?"C":"B"}))};
    expect(validWorkflowEditor(explicit)).toBe(true);
  });
  it("renders frozen command text safely and offers an explicit discard confirmation",()=>{
    const value=operation();if(value.request.kind==="create")value.request.request.definition.name="<img src=x onerror=attack()>";
    const markup=renderToStaticMarkup(<WorkflowOperationPanel operation={value} busy={false} error={null} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).toContain("&lt;img");expect(markup).not.toContain("<img src=x");expect(markup).toContain("Discard operation");expect(markup).toContain("Retry the same");
  });
});
