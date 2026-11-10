import {afterEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode,defaultRunBudgets,type PrepareWorkflowOperation,type WorkflowOperation} from "@cowork/client";
import {WorkflowInputResumePanel,workflowInputCanResume,workflowInputResumeRequest} from "./WorkflowInputResumePanel";
import {WorkflowOperationPanel,WorkflowOperationSession,WorkflowRecoveryRequired} from "./workflowJournal";
import {en} from "./i18n";

function view(){const binding={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};
  return decode("WorkflowRunView",{run:{id:"run",revision:4,last_sequence:9,state:"interrupted",created_at:1,updated_at:2,steps:2,usage:{},spec:{order_id:"order",project_id:"project",prompt:"Original",provider:null,workflow:binding,access_mode:"confirm_all",executor:"local-v2",policy_revision:1,allowed_tools:["request_input"],budgets:{...defaultRunBudgets},folders:[]}},binding,name:"Workflow",version_revision:1,mode:{mode:"sequential"},max_parallel:1,state:"interrupted",roles:[],nodes:[],approvals:[],inputs:[],input_resume_available:true,input_resume_blocked_reason:null,input_resume_preview:{available:true,blocked_reason:null,base_run_revision:4,base_workflow_revision:3,expected_checkpoint_sha256:"c".repeat(64),expected_cursor_sha256:"d".repeat(64)},completion_resume_available:false,charged_tokens:4,steps_used:2});
}
const operation=():WorkflowOperation=>({id:"operation",project_id:"project",intent_id:"intent",request_sha256:"f".repeat(64),request:{kind:"resume_input",request:{...workflowInputResumeRequest(view()),idempotency_key:"original-key"}},draft_snapshot:null,state:"pending",receipt:null,rejection:null,created_at:1,updated_at:1,acknowledged_at:null});
function accepted(value:WorkflowOperation):WorkflowOperation{return {...value,state:"accepted",updated_at:3,receipt:{kind:"input_resume_queued",run_id:"run",resume_id:value.id,generation:1,accepted_run_revision:5,accepted_workflow_revision:4,cursor_sha256:"d".repeat(64),confirmed_sequence:10,accepted_at:3}};}
const matcher=(value:WorkflowOperation)=>value.request.kind==="resume_input"&&value.request.request.run_id==="run";
const json=(value:unknown)=>new Response(JSON.stringify(value),{status:200,headers:{"Content-Type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

describe("explicit human continuation",()=>{
  it("requires bound revisions and both hashes before creating a request key",()=>{
    const original=view();expect(workflowInputCanResume(original)).toBe(true);
    for(const mutate of [(v:ReturnType<typeof view>)=>{v.run.state="running";},(v:ReturnType<typeof view>)=>{v.input_resume_preview!.base_run_revision++;},(v:ReturnType<typeof view>)=>{v.input_resume_preview!.expected_cursor_sha256=null;},(v:ReturnType<typeof view>)=>{v.input_resume_preview!.blocked_reason="workflow_clock_rollback";},(v:ReturnType<typeof view>)=>{v.input_resume_preview!.base_workflow_revision=Number.MAX_SAFE_INTEGER+1;}]){
      const current=view();mutate(current);expect(workflowInputCanResume(current)).toBe(false);expect(()=>workflowInputResumeRequest(current)).toThrow("workflowInputResumeUnavailable");
    }
    const request=workflowInputResumeRequest(original);expect(request).toMatchObject({run_id:"run",base_run_revision:4,base_workflow_revision:3,expected_checkpoint_sha256:"c".repeat(64),expected_cursor_sha256:"d".repeat(64)});
    expect(request).not.toHaveProperty("answer");expect(request).not.toHaveProperty("owner");expect(request).not.toHaveProperty("spec");
  });
  it("renders a separate explicit action and does not write during presentation",()=>{
    const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const changed=vi.fn(async()=>{});
    const markup=renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}><WorkflowInputResumePanel client={new Client("synthetic")} view={view()} t={key=>en[key]} busy={false} changed={changed}/></QueryClientProvider>);
    expect(markup).toContain(en.workflowInputResume);expect(markup).toContain(en.workflowInputResumeHint);expect(fetch).not.toHaveBeenCalled();expect(changed).not.toHaveBeenCalled();
  });
  it("recovers Pending and Accepted through reads without a new request or automatic commit",async()=>{
    for(const current of [operation(),accepted(operation())]){
      const writes=vi.fn(),build=vi.fn();vi.stubGlobal("fetch",vi.fn(async(_input:unknown,init?:RequestInit)=>{if(init?.method&&init.method!=="GET")writes();return json({items:[current],total:1,next_offset:null});}));
      const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);
      await expect(session.prepare(build)).rejects.toBeInstanceOf(WorkflowRecoveryRequired);expect(build).not.toHaveBeenCalled();expect(writes).not.toHaveBeenCalled();expect(session.operation).toEqual(current);
    }
  });
  it("retains one frozen Prepare after a lost acknowledgement and stale preview",async()=>{
    let saved:WorkflowOperation|null=null;let posts=0;
    vi.stubGlobal("fetch",vi.fn(async(_input:unknown,init?:RequestInit)=>{if(init?.method==="POST"){posts++;const body=decode("PrepareWorkflowOperation",JSON.parse(String(init.body)));saved={...operation(),intent_id:body.intent_id,request:body.request};throw new TypeError("lost ACK");}return json({items:saved?[saved]:[],total:saved?1:0,next_offset:null});}));
    const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);const current=view();const build=vi.fn(()=>({kind:"resume_input" as const,request:workflowInputResumeRequest(current)}));
    await expect(session.prepare(build)).rejects.toThrow("lost ACK");current.run.revision=99;
    expect((await session.prepare(build)).request).toEqual(saved!.request);expect(build).toHaveBeenCalledTimes(1);expect(posts).toBe(1);
  });
  it("uses a bodyless ID-only Commit and retains the exact accepted receipt after lost ACK",async()=>{
    const current=operation(),receipt=accepted(current),calls:string[]=[];
    vi.stubGlobal("fetch",vi.fn(async(input:unknown,init?:RequestInit)=>{calls.push(String(input));expect(init?.body).toBeUndefined();if(init?.method==="POST")throw new TypeError("lost commit ACK");return json(receipt);}));
    const session=new WorkflowOperationSession(new Client("fixture","http://fixture"),"project",matcher);session.attach(current);
    await expect(session.commit()).rejects.toThrow("lost commit ACK");expect(session.operation).toEqual(receipt);expect(calls).toHaveLength(2);
    expect(calls[0]).toMatch(/\/operation\/commit$/);expect(calls.every(path=>!path.includes("answer")&&!path.includes("\/runs\/"))).toBe(true);
    session.attach(receipt);expect(session.operation?.receipt).toEqual(receipt.receipt);
  });
  it("refuses a substituted cursor, authority or changed historical receipt",()=>{
    const current=operation(),original=accepted(current);const session=new WorkflowOperationSession(new Client("fixture"),"project",matcher);session.attach(current);
    for(const mutation of [(v:WorkflowOperation)=>{if(v.receipt?.kind==="input_resume_queued")v.receipt.cursor_sha256="a".repeat(64);},(v:WorkflowOperation)=>{if(v.request.kind==="resume_input")v.request.request.base_run_revision++;},(v:WorkflowOperation)=>{if(v.receipt?.kind==="input_resume_queued")v.receipt.resume_id="other";}]){
      const forged=structuredClone(original);mutation(forged);expect(()=>session.attach(forged)).toThrow("contract_mismatch");expect(session.operation).toEqual(current);
    }
    session.attach(original);const forged=structuredClone(original);if(forged.receipt?.kind==="input_resume_queued")forged.receipt.generation++;
    expect(()=>session.attach(forged)).toThrow("contract_mismatch");expect(session.operation).toEqual(original);
  });
  it("shows an accepted resume receipt without exposing an answer confirmation action",()=>{
    const markup=renderToStaticMarkup(<WorkflowOperationPanel operation={accepted(operation())} busy={false} error={undefined} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).toContain(en.workflowInputResume);expect(markup).toContain(en.workflowJournalAccepted);expect(markup).not.toContain(en.workflowInputSend);
  });
});
