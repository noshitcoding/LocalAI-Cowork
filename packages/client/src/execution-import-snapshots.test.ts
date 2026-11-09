import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,type PreparedExecutionPlanV2} from "./index";

// Public bytes from an actual native, model-free import/copy admission.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-import-plan-v2.json",import.meta.url),"utf8"));
const request=decode("PrepareImportExecutionPlanV2",fixture.prepare);
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const run=()=>decode("RunRecord",structuredClone(fixture.run));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

it("verifies the actual native request/spec hashes and commits through the common copy route",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(run()));vi.stubGlobal("fetch",fetch);
  const client=new Client("synthetic-owner","https://fixture.invalid");
  const plan=await client.prepareImportExecutionPlanV2(request);
  expect(await client.commitImportExecutionPlanV2(plan)).toEqual(run());
  expect(fetch.mock.calls[0][0]).toBe("https://fixture.invalid/api/v2/execution-plans/v2/file-imports");
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request);
  expect(fetch.mock.calls[1][0]).toBe(`https://fixture.invalid/api/v2/orders/${plan.plan.order_id}/execution-plans/v2/${plan.plan.id}/commit`);
  expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({sha256:plan.sha256});
});

it("binds upload, destination, observed replacement and original prompt to the prepared request",async()=>{
  const client=new Client("synthetic-owner");
  for(const changed of [
    {...request,request:{...request.request,prompt:"different intended action"}},
    {...request,request:{...request.request,import:{...request.request.import,path:"another.bin"}}},
    {...request,request:{...request.request,import:{...request.request.import,upload_id:"different-upload"}}},
    {...request,request:{...request.request,import:{...request.request.import,expected_sha256:"0".repeat(64)}}},
  ]){
    const fetch=vi.fn().mockResolvedValueOnce(response(prepared()));vi.stubGlobal("fetch",fetch);
    await expect(client.prepareImportExecutionPlanV2(changed)).rejects.toThrow("execution_plan_v2_response_mismatch");expect(fetch).toHaveBeenCalledTimes(1);
  }
});

it("rejects an accepted record with a model, altered upload action or missing independent copy",async()=>{
  const client=new Client("synthetic-owner");
  for(const mutate of [
    (value:ReturnType<typeof run>)=>{value.spec.prompt="other action";},
    (value:ReturnType<typeof run>)=>{delete value.spec.input_workspace;},
    (value:ReturnType<typeof run>)=>{value.spec.provider={id:"p",name:"unexpected",kind:"test",endpoint:"",model:"test",revision:1,context_tokens:8192,credential_required:false,credential_ready:true};},
    (value:ReturnType<typeof run>)=>{value.spec.file_action!.arguments={...value.spec.file_action!.arguments as object,upload_id:"other-upload"};},
  ]){
    const changed=run();mutate(changed);vi.stubGlobal("fetch",vi.fn().mockResolvedValueOnce(response(changed)));
    await expect(client.commitImportExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
  }
});

it("rejects a changed public review before sending confirmation",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("synthetic-owner");
  const plan:PreparedExecutionPlanV2=prepared();if(plan.plan.preflight.kind!=="import")throw new Error("import fixture");
  plan.plan.preflight.review.import.path="changed-display.bin";
  await expect(client.commitImportExecutionPlanV2(plan)).rejects.toThrow("execution_plan_v2_integrity");expect(fetch).not.toHaveBeenCalled();
});

it("keeps a pending import when a retirement receipt names a different order",async()=>{
  const fetch=vi.fn().mockImplementation(()=>Promise.resolve(response({order_id:"foreign-order",idempotency_key:request.request.idempotency_key,request_sha256:"0".repeat(64),retired:true})));vi.stubGlobal("fetch",fetch);
  const client=new Client("synthetic-owner");
  await expect(client.retireImportExecutionIntentV2(request.request)).rejects.toThrow("execution_plan_v2_receipt_mismatch");
  await expect(client.retireImportExecutionRequestV2(request)).rejects.toThrow("execution_plan_v2_receipt_mismatch");
});
