import {readFileSync} from "node:fs";
import {createHash} from "node:crypto";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,defaultRunBudgets,type RunRecord,type WorkflowVersion} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {answerCanRegenerate,decodeRegenerationIntent,prepareRegenerationReview,singleStartJournalKey,workflowRegenerationIntent,type RegenerationIntent} from "./regenerationReview";

const definition=decode("WorkflowDefinition",JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-dataflow.json",import.meta.url),"utf8")));
function sorted(v:unknown):unknown{if(Array.isArray(v))return v.map(sorted);if(v&&typeof v==="object")return Object.fromEntries(Object.entries(v).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([k,v])=>[k,sorted(v)]));return v;}
const sha=createHash('sha256').update(JSON.stringify(sorted(definition))).digest('hex');
const client=new Client("TRANSPORT-SECRET-ONLY","https://fixture.invalid"),parent={id:"parent",project_id:"project"};
const child={id:"child",project_id:"project",title:"New workflow answer",draft:"original prompt",revision:1,created_at:1};
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
const version=():WorkflowVersion=>({workflow_id:"workflow-one",project_id:"project",reference:{id:"version-old",revision:2,name:definition.name,sha256:sha,created_at:1},definition:structuredClone(definition)});
function source():RunRecord{
 const raw=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-plan-v2.json",import.meta.url),"utf8")).run;
 raw.spec.provider=null;raw.spec.workflow={workflow_id:"workflow-one",version_id:"version-old",definition_sha256:sha,plan_sha256:"b".repeat(64),inputs:{path:"safe.txt",sha256:"c".repeat(64),currency:"EUR",private_note:"synthetic"}};
 return decode("RunRecord",raw);
}
function intent():RegenerationIntent{return {branch:{title:"New workflow answer",through_message_id:"answer",idempotency_key:"branch-key"},...workflowRegenerationIntent(source(),{
 order_id:parent.id,base_revision:8,idempotency_key:"workflow-start",access_mode:"confirm_mutations",budgets:structuredClone(defaultRunBudgets),folders:[],native_tools:[],mcp_tools:[],environment:null},"original prompt")};}
function memory(){const values=new Map<string,string>();return {values,storage:{getItem:(k:string)=>values.get(k)??null,setItem:(k:string,v:string)=>{values.set(k,v);},removeItem:(k:string)=>{values.delete(k);}}};}
afterEach(()=>vi.unstubAllGlobals());

it("hands the original immutable workflow version and input to the normal child review without carrying its old plan",async()=>{
 const {storage,values}=memory(),original=intent();
 const fetch=vi.fn().mockResolvedValueOnce(response(version())).mockResolvedValueOnce(response(child));vi.stubGlobal("fetch",fetch);
 await prepareRegenerationReview(client,parent,original,storage);
 const saved=new PersistentRequestLatch(singleStartJournalKey(client,"project","child"),v=>decode("RunRequest",v),storage).prepare(()=>{throw new Error("must recover");});
 expect(saved).toEqual({...original.template,order_id:"child",base_revision:1});expect("kind" in saved&&saved.kind).toBe("workflow");
 expect("provider_id" in saved).toBe(false);expect("kind" in saved&&saved.expected_binding).toBeUndefined();
 expect(fetch.mock.calls.map(c=>String(c[0]))).toEqual(["https://fixture.invalid/api/v2/projects/project/workflows/workflow-one/versions/version-old","https://fixture.invalid/api/v2/orders/parent/regenerate"]);
 expect([...values.values()].join("")).not.toContain("TRANSPORT-SECRET-ONLY");
});

it("keeps the exact workflow intent across lost branch acknowledgement and reload",async()=>{
 const {storage}=memory(),original=intent();
 const fetch=vi.fn().mockResolvedValueOnce(response(version())).mockRejectedValueOnce(new TypeError("lost branch ACK")).mockResolvedValueOnce(response(version())).mockResolvedValueOnce(response(child));vi.stubGlobal("fetch",fetch);
 const parentJournal=new PersistentRequestLatch("parent-intent",decodeRegenerationIntent,storage);parentJournal.prepare(()=>original);
 await expect(prepareRegenerationReview(client,parent,parentJournal.prepare(()=>original),storage)).rejects.toThrow("lost branch ACK");
 const recovered=new PersistentRequestLatch("parent-intent",decodeRegenerationIntent,storage);
 await prepareRegenerationReview(client,parent,recovered.prepare(()=>{throw new Error("must recover");}),storage);
 expect(recovered.pending).toBe(true);expect(fetch.mock.calls.filter(c=>String(c[0]).endsWith('/regenerate')).map(c=>JSON.parse(c[1].body))).toEqual([original.branch,original.branch]);
});

it.each(["definition","version","project"])("rejects a changed %s before creating any branch or child start",async field=>{
 const {storage,values}=memory(),changed=version();
 if(field==="definition")changed.reference.sha256="f".repeat(64);else if(field==="version")changed.reference.id="version-latest";else changed.project_id="foreign";
 const fetch=vi.fn().mockResolvedValue(response(changed));vi.stubGlobal("fetch",fetch);
 await expect(prepareRegenerationReview(client,parent,intent(),storage)).rejects.toThrow("execution_plan_original_mismatch");expect(fetch).toHaveBeenCalledTimes(1);expect(values.size).toBe(0);
});

it("does not reinterpret legacy single journals or accept unbound workflow metadata",()=>{
 const single={branch:intent().branch,template:{order_id:"parent",base_revision:8,idempotency_key:"old-single",prompt:"prompt",provider_id:"model",provider_revision:1}};
 expect(JSON.stringify(decodeRegenerationIntent(single))).toBe(JSON.stringify(single));
 const missing=intent();delete missing.workflow_source;expect(()=>decodeRegenerationIntent(missing)).toThrow("execution_plan_original_mismatch");
 const stale=intent();if(!("kind" in stale.template))throw new Error("workflow");stale.template.expected_binding=source().spec.workflow;
 expect(()=>decodeRegenerationIntent(stale)).toThrow("execution_plan_original_mismatch");
});

it("freezes workflow data and project grants without copying an old chat capability",()=>{
 const run=source(),project={id:"project-folder",project_id:"project",name:"Selected",resource_id:"r",revision:"rev",writable:true};
 const context={order_id:"parent",base_revision:1,idempotency_key:"key",folders:[project,{...project,id:"workspace:parent",order_id:"parent"}]};
 const result=workflowRegenerationIntent(run,context,"prompt");
 (run.spec.workflow!.inputs as {currency:string}).currency="USD";project.name="later";
 expect(result.template.folders).toEqual([{...project,name:"Selected"}]);expect("kind" in result.template&&result.template.inputs).toMatchObject({currency:"EUR"});
 expect(answerCanRegenerate(run)).toBe(true);run.spec.input_workspace=null;expect(answerCanRegenerate(run)).toBe(false);
});

it("does not start a workflow through a requested process or connector fallback",()=>{
 for(const options of [{native_tools:["execute_program"]},{mcp_tools:[{server_id:"mcp",server_revision:1,tool_name:"tool"}]}]){
  expect(()=>workflowRegenerationIntent(source(),{order_id:"parent",base_revision:1,idempotency_key:"key",...options},"prompt")).toThrow("snapshot_process_or_connector_adapter_required");
 }
});

it("refuses unsafe numeric input instead of silently rounding a native workflow value",()=>{
 for(const value of [Number.MAX_SAFE_INTEGER+1,Infinity,NaN]){
  const run=source();run.spec.workflow!.inputs={count:value};
  expect(()=>workflowRegenerationIntent(run,{order_id:"parent",base_revision:1,idempotency_key:"key"},"prompt")).toThrow("regeneration_workflow_input_transport_required");
 }
});

it("hands an exact native source reference through the durable child journal without serializing its rounded inputs",async()=>{
 const run=source();run.spec.workflow!.inputs={amount:9007199254740992};
 const text='{"amount":9007199254740993}',snapshot={source:{run_id:run.id,sha256:createHash("sha256").update(text).digest("hex")},project_id:run.spec.project_id,
  workflow_id:run.spec.workflow!.workflow_id,version_id:run.spec.workflow!.version_id,definition_sha256:sha,canonical_json:text};
 const original={branch:intent().branch,...workflowRegenerationIntent(run,{order_id:"parent",base_revision:1,idempotency_key:"exact-input"},"prompt",snapshot)};
 expect("kind" in original.template&&original.template.inputs).toBeUndefined();
 expect("kind" in original.template&&original.template.input_source).toEqual(snapshot.source);
 expect(JSON.stringify(original)).not.toContain("9007199254740992");
 const {storage}=memory();vi.stubGlobal("fetch",vi.fn().mockResolvedValueOnce(response(version())).mockResolvedValueOnce(response(snapshot)).mockResolvedValueOnce(response(child)));
 await prepareRegenerationReview(client,parent,original,storage);
 const saved=new PersistentRequestLatch(singleStartJournalKey(client,"project","child"),v=>decode("RunRequest",v),storage).prepare(()=>{throw new Error("recover");});
 expect("kind" in saved&&saved.input_source).toEqual(snapshot.source);
});

it("rechecks the exact native input reference before creating a branch",async()=>{
 const original=intent();if(!("kind" in original.template))throw new Error("workflow");
 original.template.inputs=undefined;original.template.input_source={run_id:"original-run",sha256:"a".repeat(64)};
 const snapshot={source:{run_id:"original-run",sha256:"b".repeat(64)},project_id:"project",workflow_id:"workflow-one",version_id:"version-old",definition_sha256:sha,canonical_json:'{}'};
 const fetch=vi.fn().mockResolvedValueOnce(response(version())).mockResolvedValueOnce(response(snapshot));vi.stubGlobal("fetch",fetch);
 await expect(prepareRegenerationReview(client,parent,original,memory().storage)).rejects.toThrow("workflow_input_source_mismatch");
 expect(fetch).toHaveBeenCalledTimes(2);
});

it("blocks order-bound folder rights, including nested workflows, before branching",async()=>{
 for(const nested of [false,true]){
  const {storage,values}=memory(),value=version();
  if(nested)value.definition.subflows={child:{name:"nested",dependencies:[],definition:{...structuredClone(definition),rights:{tools:[],folders:["workspace:old-order"]}},result:{step_id:"report",pointer:""},rights:{tools:[],folders:[]},budgets:defaultRunBudgets}};
  else value.definition.roles[0].rights.folders=["workspace:old-order"];
  const fetch=vi.fn().mockResolvedValue(response(value));vi.stubGlobal("fetch",fetch);
  await expect(prepareRegenerationReview(client,parent,intent(),storage)).rejects.toThrow("regeneration_workflow_workspace_binding_required");expect(fetch).toHaveBeenCalledTimes(1);expect(values.size).toBe(0);
 }
});
