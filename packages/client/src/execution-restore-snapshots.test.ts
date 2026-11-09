import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,type PreparedExecutionPlanV2} from "./index";

// Captured from a real native copy/restore admission; public fields only.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-restore-plan-v2.json",import.meta.url),"utf8"));
const request=decode("PrepareRestoreExecutionPlanV2",fixture.prepare);
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const run=()=>decode("RunRecord",structuredClone(fixture.run));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

it("verifies the actual model-free spec hash and uses the common copy commit route",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(run()));vi.stubGlobal("fetch",fetch);
  const client=new Client("synthetic-owner","https://fixture.invalid");
  const plan=await client.prepareRestoreExecutionPlanV2(request);
  expect(await client.commitRestoreExecutionPlanV2(plan)).toEqual(run());
  expect(fetch.mock.calls[0][0]).toBe("https://fixture.invalid/api/v2/execution-plans/v2/file-restorations");
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request);
  expect(fetch.mock.calls[1][0]).toBe(`https://fixture.invalid/api/v2/orders/${plan.plan.order_id}/execution-plans/v2/${plan.plan.id}/commit`);
  expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({sha256:plan.sha256});
});

it("rejects a different requested version or destination despite an otherwise valid public plan",async()=>{
  const client=new Client("synthetic-owner");
  for(const restore of [{...request.request.restore,path:"another-target.txt"},{...request.request.restore,source:{...request.request.restore.source,version:"text_before" as const}},{...request.request.restore,expected_source_sha256:"0".repeat(64)}]){
    const fetch=vi.fn().mockResolvedValueOnce(response(prepared()));vi.stubGlobal("fetch",fetch);
    await expect(client.prepareRestoreExecutionPlanV2({...request,request:{...request.request,restore}})).rejects.toThrow("execution_plan_v2_response_mismatch");
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});

it("rejects an accepted record that changes content, adds a model or loses its independent copy",async()=>{
  const client=new Client("synthetic-owner");
  for(const mutate of [
    (value:ReturnType<typeof run>)=>{value.spec.prompt="other action";},
    (value:ReturnType<typeof run>)=>{delete value.spec.input_workspace;},
    (value:ReturnType<typeof run>)=>{value.spec.provider={id:"p",name:"unexpected",kind:"test",endpoint:"",model:"test",revision:1,context_tokens:8192,credential_required:false,credential_ready:true};},
    (value:ReturnType<typeof run>)=>{value.spec.file_action!.arguments={...value.spec.file_action!.arguments as object,path:"other.txt"};},
  ]){
    const changed=run();mutate(changed);vi.stubGlobal("fetch",vi.fn().mockResolvedValueOnce(response(changed)));
    await expect(client.commitRestoreExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
  }
});

it("rejects a changed public display or hash before sending any confirmation",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("synthetic-owner");
  const plan:PreparedExecutionPlanV2=prepared();if(plan.plan.preflight.kind!=="restore")throw new Error("restore fixture");
  plan.plan.preflight.review.restore.path="changed-display.txt";
  await expect(client.commitRestoreExecutionPlanV2(plan)).rejects.toThrow("execution_plan_v2_integrity");expect(fetch).not.toHaveBeenCalled();
});
