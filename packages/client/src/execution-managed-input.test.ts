import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,type PreparedExecutionPlanV2} from "./index";

// Exported by the actual Rust owner/store/broker test. Public fixture keys only.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-managed-input.json",import.meta.url),"utf8"));
const request=decode("PrepareOwnerExecutionPlanV2",fixture.prepare);
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());
const sorted=(value:unknown):unknown=>Array.isArray(value)?value.map(sorted):value&&typeof value==="object"?Object.fromEntries(Object.entries(value).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([key,item])=>[key,sorted(item)])):value;
async function rehash(value:PreparedExecutionPlanV2){value.canonical_json=JSON.stringify(sorted(value.plan));value.sha256=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value.canonical_json)))).map(b=>b.toString(16).padStart(2,"0")).join("");return value;}

it("accepts the exact native managed target, qualified environment and confirmed copy",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(fixture.run));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner"),plan=await client.prepareExecutionPlanV2(request);
  expect(plan.plan.target.kind).toBe("managed_runner");expect(plan.plan.boundary.process_network.kind).toBe("denied");
  expect(await client.commitExecutionPlanV2(plan)).toEqual(fixture.run);
  expect(JSON.parse(fetch.mock.calls[0][1].body).target).toEqual({kind:"registered_runner"});
  expect(fetch.mock.calls).toHaveLength(2);
});

it("rejects invalid target, program and network scope even with a recomputed public checksum",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  const changes:((p:PreparedExecutionPlanV2)=>void)[]=[
    p=>{if(p.plan.target.kind==="managed_runner")p.plan.target.target.revision=0;},
    p=>{if(p.plan.target.kind==="managed_runner")p.plan.target.runner.generation+=1;},
    p=>{if(p.plan.target.kind==="managed_runner")p.plan.target.programs.probe="/host/original";},
    p=>{p.plan.boundary.process_network={kind:"user_rights"};},
    p=>{p.plan.boundary.environment=null;},
    p=>{p.plan.boundary.folders[0].writable=false;},
  ];
  for(const change of changes){const p=prepared();change(p);fetch.mockResolvedValueOnce(response(await rehash(p)));
    await expect(client.prepareExecutionPlanV2(request)).rejects.toThrow("execution_plan_v2_integrity");}
  expect(fetch.mock.calls.every(([url])=>!String(url).endsWith("/commit"))).toBe(true);
});

it("requires the requested destination and retains the environment in its original receipt",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared()));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.prepareExecutionPlanV2({...request,target:{kind:"broker_controller"}})).rejects.toThrow("execution_plan_v2_response_mismatch");
  const run=structuredClone(fixture.run);delete run.spec.environment;fetch.mockResolvedValueOnce(response(run));
  await expect(client.commitExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
});
