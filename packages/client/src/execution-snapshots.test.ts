import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,type PreparedExecutionPlanV2} from "./index";

// Independent Python JSON/SHA-256 fixture also decoded and checked by Rust.
// These public codec bytes contain no credentials and grant no execution.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-plan-v2.json",import.meta.url),"utf8"));
const request=decode("PrepareOwnerExecutionPlanV2",fixture.prepare);
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const run=()=>decode("RunRecord",structuredClone(fixture.run));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

it("checks exact public plan bytes including decimal lexemes and starts only its reviewed copy",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(run()));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner","https://fixture.invalid");
  const plan=await client.prepareExecutionPlanV2(request);expect(plan.canonical_json).toContain('"temperature":0.0');
  expect(JSON.stringify(plan.plan)).toContain('"temperature":0');
  expect(await client.commitExecutionPlanV2(plan)).toEqual(run());
  expect(fetch.mock.calls[0][0]).toBe("https://fixture.invalid/api/v2/execution-plans/v2");
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request);
  expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({sha256:plan.sha256});
  expect(fetch.mock.calls[1][0]).toBe(`https://fixture.invalid/api/v2/orders/order/execution-plans/v2/${plan.plan.id}/commit`);
});

it("rejects changed display, raw bytes, snapshot and native scope before any confirmation",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  for(const mutate of [
    (p:PreparedExecutionPlanV2)=>{p.plan.boundary.process_access="user_rights";},
    (p:PreparedExecutionPlanV2)=>{p.canonical_json=p.canonical_json.replace('"temperature":0.0','"temperature":0');},
    (p:PreparedExecutionPlanV2)=>{p.plan.input.manifest.total_bytes=1;},
    (p:PreparedExecutionPlanV2)=>{p.plan.workspace.id="different copy";},
    (p:PreparedExecutionPlanV2)=>{p.sha256="0".repeat(64);},
  ]){
    const p=prepared();mutate(p);fetch.mockResolvedValueOnce(response(p));
    await expect(client.prepareExecutionPlanV2(request)).rejects.toThrow("execution_plan_v2_integrity");
  }
  expect(fetch.mock.calls.every(([url])=>!String(url).endsWith("/commit"))).toBe(true);
  fetch.mockClear();const p=prepared();p.plan.boundary.executor="another target";
  await expect(client.commitExecutionPlanV2(p)).rejects.toThrow("execution_plan_v2_integrity");expect(fetch).not.toHaveBeenCalled();
});

it("rejects an unrelated original request and a run that loses the confirmed copy binding",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared()));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.prepareExecutionPlanV2({...request,request:{...request.request,idempotency_key:"other"}})).rejects.toThrow("execution_plan_v2_response_mismatch");
  const changed=run();delete changed.spec.input_workspace;fetch.mockResolvedValueOnce(response(changed));
  await expect(client.commitExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
});

it("keeps inspection and retirement monotonic and replays no lost commit automatically",async()=>{
  const p=prepared(),retired={...p,retired:true};
  const fetch=vi.fn().mockResolvedValueOnce(response(p)).mockResolvedValueOnce(response(retired)).mockResolvedValueOnce(response(p));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");expect(await client.inspectExecutionPlanV2(p)).toEqual(p);
  expect(await client.retireExecutionPlanV2(p)).toEqual(retired);
  await expect(client.inspectExecutionPlanV2(retired)).rejects.toThrow("execution_plan_v2_receipt_mismatch");
  fetch.mockReset();fetch.mockRejectedValue(new TypeError("lost accepted response"));
  await expect(client.commitExecutionPlanV2(p)).rejects.toThrow("lost accepted response");expect(fetch).toHaveBeenCalledTimes(1);
  await expect(client.commitExecutionPlanV2(p)).rejects.toThrow("lost accepted response");expect(fetch.mock.calls[0]).toEqual(fetch.mock.calls[1]);
});
