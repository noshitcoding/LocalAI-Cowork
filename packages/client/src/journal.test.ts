import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,decode,RequestError} from "./index";
const content={editor_format:"cowork.workflow-editor.v1",data:{name:"",budgets:{max_steps:-1}},origin:null,archived:false};
const save={id:"draft",base_revision:2,idempotency_key:"save-1",content};
const current={id:"draft",project_id:"project",revision:3,snapshot_id:"snapshot",sha256:"a".repeat(64),content,updated_at:42};
const snapshot={id:"snapshot",draft_id:"draft",project_id:"project",base_revision:2,revision:3,content,sha256:"a".repeat(64),conflict:false,created_at:42};
const request={kind:"start" as const,request:{kind:"workflow" as const,order_id:"order",base_revision:1,idempotency_key:"start",prompt:"Review",workflow_id:"workflow",version_id:"version"}};
const prepare={intent_id:"intent",request,draft_snapshot:null};
const op={id:"operation",project_id:"project",intent_id:"intent",request_sha256:"b".repeat(64),request,draft_snapshot:null,state:"pending",receipt:null,rejection:null,created_at:42,updated_at:42,acknowledged_at:null};
const failure={code:"revision_conflict",message:"Changed request",retryable:false,correlation_id:"correlation"};
describe("durable workflow draft and operation transport",()=>{
 afterEach(()=>vi.unstubAllGlobals());
 it("preserves incomplete editor content while rejecting supplied execution authority",()=>{
  expect(decode("SaveWorkflowDraft",save)).toEqual(save);
  for(const authority of ["scope","grant","state","receipt","spec"]){
   expect(()=>decode("PrepareWorkflowOperation",{...prepare,[authority]:{}})).toThrow("contract_mismatch");
   expect(()=>decode("PrepareWorkflowOperation",{...prepare,request:{...request,[authority]:{}}})).toThrow("contract_mismatch");
  }
 });
 it("returns the committed incoming conflict copy on409 instead of forgetting it",async()=>{
  const own={...snapshot,id:"conflict-snapshot",conflict:true};const response={outcome:"conflict",draft:current,snapshot:own};
  const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify(response),{status:409}));vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").saveWorkflowDraft("project",save)).resolves.toEqual(response);
  expect(fetch).toHaveBeenCalledTimes(1);expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(save);
 });
 it("keeps changed-key conflicts as structured API errors and rejects cross-project draft receipts",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify(failure),{status:409}))
   .mockResolvedValueOnce(new Response(JSON.stringify({outcome:"saved",draft:{...current,project_id:"other"},snapshot}),{status:200}));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");await expect(client.saveWorkflowDraft("project",save)).rejects.toBeInstanceOf(RequestError);
  await expect(client.saveWorkflowDraft("project",save)).rejects.toThrow("workflow_draft_integrity");expect(fetch).toHaveBeenCalledTimes(2);
 });
 it("checks the content that was sent before awaiting the response, even if the caller changes its object",async()=>{
  const mutable=structuredClone(save);let finish!:(value:Response)=>void;
  const fetch=vi.fn().mockReturnValue(new Promise<Response>(resolve=>{finish=resolve;}));vi.stubGlobal("fetch",fetch);
  const result=new Client("owner").saveWorkflowDraft("project",mutable);
  mutable.base_revision=90;mutable.content.data.name="changed later";
  finish(new Response(JSON.stringify({outcome:"saved",draft:current,snapshot}),{status:200}));
  await expect(result).resolves.toMatchObject({outcome:"saved"});expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(save);
 });
 it("rejects a successful receipt that silently substitutes another editor value",async()=>{
  const other={...content,data:{name:"different value"}};
  vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify({outcome:"saved",draft:{...current,content:other},snapshot:{...snapshot,content:other}}),{status:200})));
  await expect(new Client("owner").saveWorkflowDraft("project",save)).rejects.toThrow("workflow_draft_integrity");
 });
 it("does not automatically repeat a lost preparation or commit and retries only the saved operation ID",async()=>{
  const fetch=vi.fn().mockRejectedValue(new TypeError("lost acknowledgement"));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.prepareWorkflowOperation("project",prepare)).rejects.toThrow("lost acknowledgement");expect(fetch).toHaveBeenCalledTimes(1);
  await expect(client.prepareWorkflowOperation("project",prepare)).rejects.toThrow("lost acknowledgement");expect(fetch.mock.calls.slice(0,2).map(call=>JSON.parse(call[1].body))).toEqual([prepare,prepare]);
  await expect(client.commitWorkflowOperation("project","op/old")).rejects.toThrow("lost acknowledgement");expect(fetch).toHaveBeenCalledTimes(3);
  await expect(client.commitWorkflowOperation("project","op/old")).rejects.toThrow("lost acknowledgement");expect(fetch).toHaveBeenCalledTimes(4);
  for(const call of fetch.mock.calls.slice(2)){expect(call[0]).toBe("/api/v2/projects/project/workflow-operations/op%2Fold/commit");expect(call[1].body).toBeUndefined();}
 });
 it("loads scoped unacknowledged recovery pages and sends discard/acknowledge by ID without another command",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({items:[op],total:1,next_offset:null}),{status:200}))
   .mockResolvedValueOnce(new Response(JSON.stringify({...op,state:"discarded"}),{status:200}))
   .mockResolvedValueOnce(new Response(JSON.stringify({...op,state:"discarded",acknowledged_at:43}),{status:200}));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await client.workflowOperations("a/b",20,10);await client.discardWorkflowOperation("project","operation");await client.acknowledgeWorkflowOperation("project","operation");
  expect(fetch.mock.calls[0][0]).toBe("/api/v2/projects/a%2Fb/workflow-operations?offset=20&limit=10&include_acknowledged=false");
  for(const call of fetch.mock.calls.slice(1))expect(call[1].body).toBeUndefined();
 });
});
