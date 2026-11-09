import {afterEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,defaultRunBudgets,type WorkflowDefinition,type WorkflowDraftSnapshot,type WorkflowOperation,type WorkflowVersion} from "@cowork/client";
import {en} from "./i18n";
import {WorkflowSnapshotRecovery} from "./WorkflowSnapshotPanel";
import {readWorkflowOperationSnapshot,independentSnapshotContent,prepareIndependentSnapshotCopy,WorkflowSnapshotReader,workflowDefinitionKey} from "./workflowSnapshotRecovery";
import {workflowDraftKey,workflowEditorFormat} from "./workflowJournal";

const definition=():WorkflowDefinition=>({format_version:1,name:"Original",mode:{mode:"parallel",max_parallel:2},failure:"continue_independent",rights:{tools:["read_artifact","create_artifact"],folders:[]},budgets:{...defaultRunBudgets},roles:[{id:"Agent",instructions:"Original instructions",provider_id:"original-provider",provider_revision:2,rights:{tools:["read_artifact","create_artifact"],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],steps:[{id:"step",role_id:"Agent",goal:"Original goal",dependencies:[],rights:{tools:["read_artifact","create_artifact"],folders:[]},budgets:{...defaultRunBudgets}}]});
function fixture(){
  const value=definition();
  const snapshot:WorkflowDraftSnapshot={id:"snapshot",draft_id:"old-draft",project_id:"project",base_revision:1,revision:2,sha256:"b".repeat(64),conflict:false,created_at:2,content:{editor_format:workflowEditorFormat,data:value,origin:{workflow_id:"workflow",version_id:"original-version",base_revision:1},archived:false}};
  const operation:WorkflowOperation={id:"operation",project_id:"project",intent_id:"intent",request_sha256:"a".repeat(64),request:{kind:"publish",workflow_id:"workflow",request:{base_revision:1,idempotency_key:"original-key",definition:structuredClone(value)}},draft_snapshot:{draft_id:snapshot.draft_id,snapshot_id:snapshot.id,sha256:snapshot.sha256},state:"rejected",receipt:null,rejection:{code:"revision_conflict",message:"Head changed"},created_at:2,updated_at:3,acknowledged_at:null};
  const version:WorkflowVersion={project_id:"project",workflow_id:"workflow",reference:{id:"original-version",revision:1,name:"Original",sha256:"d".repeat(64),created_at:1},definition:definition()};
  return {value,snapshot,operation,version};
}
const response=(value:unknown,status=200)=>new Response(JSON.stringify(value),{status,headers:{"Content-Type":"application/json"}});
function serve(value:ReturnType<typeof fixture>){
  const paths:string[]=[];
  const fetch=vi.fn(async(input:unknown,init?:RequestInit)=>{
    const path=String(input);paths.push(path);expect(init?.method).toBe("GET");
    if(path.endsWith("/workflow-operations/operation"))return response(value.operation);
    if(path.endsWith("/workflow-drafts/old-draft/snapshots/snapshot"))return response(value.snapshot);
    if(path.endsWith("/workflows/workflow/versions/original-version"))return response(value.version);
    throw new Error("Unexpected latest-head read or write");
  });
  vi.stubGlobal("fetch",fetch);return {paths,fetch,client:new Client("synthetic","http://fixture")};
}
afterEach(()=>vi.unstubAllGlobals());

describe("historical operation-bound snapshot recovery",()=>{
  it("reads the exact old snapshot and original version without latest-head reads, writes or acknowledgement",async()=>{
    const data=fixture();const original=structuredClone(data);
    const frozen=data.operation.request;if(frozen.kind!=="publish")throw new Error("fixture");
    delete frozen.request.definition.budgets.max_total_tokens;
    for(const role of frozen.request.definition.roles){delete role.budgets.max_total_tokens;role.rights.tools.reverse();}
    for(const step of frozen.request.definition.steps)delete step.budgets.max_total_tokens;
    const {client,paths}=serve(data);
    const result=await readWorkflowOperationSnapshot(client,"project",data.operation);
    expect(result.reason).toBeNull();expect(result.snapshot?.content).toEqual(original.snapshot.content);
    expect(result.version?.reference.id).toBe("original-version");
    expect(paths).toHaveLength(3);expect(paths[0]).toMatch(/workflow-operations/);
    const input=prepareIndependentSnapshotCopy(result);
    expect(input.base_revision).toBe(0);expect(input.id).not.toBe(data.snapshot.draft_id);
    expect(input.content).toEqual(original.snapshot.content);
    expect(input.content.origin).toEqual({workflow_id:"workflow",version_id:"original-version",base_revision:1});
    expect((input.content.data as WorkflowDefinition).roles[0].provider_id).toBe("original-provider");
    expect(data.snapshot).toEqual(original.snapshot);expect(data.operation.acknowledged_at).toBeNull();
  });

  it("compares domain defaults and set semantics without reordering roles, steps or dependency vectors",()=>{
    const left=definition(),right=definition();
    delete right.budgets.max_total_tokens;delete right.roles[0].budgets.max_total_tokens;delete right.steps[0].budgets.max_total_tokens;
    right.rights.tools=["create_artifact","read_artifact","read_artifact"];right.roles[0].rights.tools.reverse();
    expect(workflowDefinitionKey(left)).toBe(workflowDefinitionKey(right));
    right.roles.push({...right.roles[0],id:"Other"});left.roles.push({...left.roles[0],id:"Other"});right.roles.reverse();
    expect(workflowDefinitionKey(left)).not.toBe(workflowDefinitionKey(right));
    right.roles.reverse();left.steps[0].dependencies=["first","second"];right.steps[0].dependencies=["second","first"];
    expect(workflowDefinitionKey(left)).not.toBe(workflowDefinitionKey(right));
  });

  it.each([
    ["foreign operation", (d:ReturnType<typeof fixture>)=>{d.operation.project_id="foreign";}],
    ["wrong snapshot id", (d:ReturnType<typeof fixture>)=>{d.snapshot.id="other";}],
    ["wrong draft id", (d:ReturnType<typeof fixture>)=>{d.snapshot.draft_id="other";}],
    ["foreign snapshot", (d:ReturnType<typeof fixture>)=>{d.snapshot.project_id="foreign";}],
    ["wrong hash reference", (d:ReturnType<typeof fixture>)=>{d.snapshot.sha256="c".repeat(64);}],
    ["changed definition", (d:ReturnType<typeof fixture>)=>{(d.snapshot.content.data as WorkflowDefinition).roles[0].instructions="Other instructions";}],
    ["wrong origin workflow", (d:ReturnType<typeof fixture>)=>{d.snapshot.content.origin!.workflow_id="foreign";}],
    ["wrong origin base", (d:ReturnType<typeof fixture>)=>{d.snapshot.content.origin!.base_revision=8;}],
    ["foreign original version", (d:ReturnType<typeof fixture>)=>{d.version.project_id="foreign";}],
    ["wrong original version id", (d:ReturnType<typeof fixture>)=>{d.version.reference.id="other";}]
  ])("keeps %s read-only instead of creating a replacement binding",async(_name,change)=>{
    const data=fixture(),expected=structuredClone(data.operation);change(data);const {client}=serve(data);
    const result=await readWorkflowOperationSnapshot(client,"project",expected);
    expect(result.reason).not.toBeNull();expect(()=>prepareIndependentSnapshotCopy(result)).toThrow("workflowSnapshotReadOnly");
  });

  it("requires current terminal state and retains unknown formats without any copy or acknowledgement",async()=>{
    const data=fixture(),expected=structuredClone(data.operation);data.operation.state="pending";data.operation.rejection=null;
    const {client,paths}=serve(data);const pending=await readWorkflowOperationSnapshot(client,"project",expected);
    expect(pending.reason).toBe("pending");expect(paths).toHaveLength(1);expect(()=>independentSnapshotContent(pending)).toThrow();
    const props={client,project:"project",operation:data.operation,busy:false,blocked:false,copied:false,t:(key:keyof typeof en)=>en[key],onCopy:vi.fn(),onExport:vi.fn(),summary:()=>null};
    expect(renderToStaticMarkup(<WorkflowSnapshotRecovery {...props}/>)).toBe("");
    data.operation.state="rejected";data.snapshot.content.editor_format="future-format";data.snapshot.content.data={opaque:"Retained <script>data</script>"};
    const unknown=await readWorkflowOperationSnapshot(client,"project",data.operation);
    expect(unknown.reason).toBe("unsupported");expect(unknown.snapshot?.content).toEqual(data.snapshot.content);
    expect(()=>prepareIndependentSnapshotCopy(unknown)).toThrow();
  });

  it("fences delayed old contexts and survives StrictMode setup-cleanup-setup without stale callbacks",async()=>{
    const data=fixture();let release!:(value:Response)=>void;let first=true;
    vi.stubGlobal("fetch",vi.fn(async(input:unknown)=>{
      if(String(input).endsWith("/workflow-operations/operation")&&first){first=false;return new Promise<Response>(resolve=>{release=resolve;});}
      return response(String(input).includes("/snapshots/")?data.snapshot:String(input).includes("/versions/")?data.version:data.operation);
    }));
    const reader=new WorkflowSnapshotReader(new Client("synthetic","http://fixture"),"project",data.operation);
    reader.activate();const stale=reader.read();reader.dispose();reader.activate();
    const current=await reader.read();expect(current?.reason).toBeNull();
    release(response(data.operation));expect(await stale).toBeNull();
    reader.dispose();expect(await reader.read()).toBeNull();
  });

  it("keeps one frozen independent CAS0 input across quota rejection, lost Save acknowledgement and exact retry",async()=>{
    const data=fixture();const {client}=serve(data);
    const restored=await readWorkflowOperationSnapshot(client,"project",data.operation);const input=prepareIndependentSnapshotCopy(restored);
    const frozen=structuredClone(input);const received:string[]=[];let phase="quota";let accepted:unknown;
    vi.stubGlobal("fetch",vi.fn(async(path:unknown,init?:RequestInit)=>{
      expect(String(path)).toMatch(/workflow-drafts$/);expect(init?.method).toBe("PUT");received.push(String(init?.body));
      if(phase==="quota"){phase="lost";return response({code:"budget_exceeded",message:"Snapshot quota",retryable:false,correlation_id:"synthetic"},429);}
      const snapshot={...data.snapshot,id:"new-snapshot",draft_id:input.id,base_revision:0,revision:1,content:frozen.content,conflict:false};
      accepted={outcome:"saved",draft:{id:input.id,project_id:"project",revision:1,snapshot_id:snapshot.id,sha256:snapshot.sha256,content:frozen.content,updated_at:4},snapshot};
      if(phase==="lost"){phase="replay";throw new TypeError("lost save acknowledgement");}
      return response(accepted);
    }));
    await expect(client.saveWorkflowDraft("project",input)).rejects.toMatchObject({details:{code:"budget_exceeded"}});
    await expect(client.saveWorkflowDraft("project",input)).rejects.toThrow("lost save");
    const confirmed=await client.saveWorkflowDraft("project",input);
    expect(confirmed.outcome).toBe("saved");expect(received).toEqual([received[0],received[0],received[0]]);
    expect(JSON.parse(received[0])).toEqual(frozen);expect(input).toEqual(frozen);
    expect(workflowDraftKey(data.snapshot.content)).toBe(workflowDraftKey(frozen.content));
    expect(data.operation.acknowledged_at).toBeNull();
  });

  it("retains incoming copy content on CAS conflict and never replaces the original publication base",async()=>{
    const data=fixture();const {client}=serve(data);const restored=await readWorkflowOperationSnapshot(client,"project",data.operation);
    const input=prepareIndependentSnapshotCopy(restored);
    const serverContent={...input.content,data:{...definition(),name:"Concurrent copy head"}};
    vi.stubGlobal("fetch",vi.fn(async()=>response({outcome:"conflict",
      draft:{id:input.id,project_id:"project",revision:1,snapshot_id:"server-snapshot",sha256:"c".repeat(64),content:serverContent,updated_at:4},
      snapshot:{...data.snapshot,id:"incoming-copy",draft_id:input.id,base_revision:0,revision:1,content:input.content,conflict:true}},409)));
    const result=await client.saveWorkflowDraft("project",input);expect(result.outcome).toBe("conflict");
    expect(result.snapshot.content).toEqual(data.snapshot.content);expect(result.draft.content).toEqual(serverContent);
    expect(result.snapshot.content.origin?.base_revision).toBe(1);
    expect((result.snapshot.content.data as WorkflowDefinition).roles[0].provider_revision).toBe(2);
  });
});
