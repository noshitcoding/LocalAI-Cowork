import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,defaultRunBudgets,type PreparedExecutionPlanV2} from "./index";

// Exported by the real Rust document target test. Public codec bytes convey no authority.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-document-input.json",import.meta.url),"utf8"));
const request=decode("PrepareOwnerExecutionPlanV2",fixture.prepare);
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
// Actual private model-free OCR browser response; public codec data, no credentials.
const ocrBudget=JSON.parse(readFileSync(new URL("./fixtures/execution-document-ocr-budget.json",import.meta.url),"utf8"));
afterEach(()=>vi.unstubAllGlobals());
const sorted=(value:unknown):unknown=>Array.isArray(value)?value.map(sorted):value&&typeof value==="object"?Object.fromEntries(Object.entries(value).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([key,item])=>[key,sorted(item)])):value;
async function rehash(value:PreparedExecutionPlanV2){value.canonical_json=JSON.stringify(sorted(value.plan));value.sha256=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value.canonical_json)))).map(b=>b.toString(16).padStart(2,"0")).join("");return value;}

it("accepts the actual OCR workflow's stricter limits and preserves the requested readonly scope",async()=>{
  const input=decode("PrepareOwnerExecutionPlanV2",ocrBudget.prepare),expected=decode("PreparedExecutionPlanV2",ocrBudget.prepared);
  const fetch=vi.fn().mockResolvedValueOnce(response(expected));vi.stubGlobal("fetch",fetch);
  const plan=await new Client("owner").prepareExecutionPlanV2(input);
  if(plan.plan.preflight.kind!=="owner")throw new Error("owner OCR fixture");
  expect(plan.plan.preflight.kind).toBe("owner");expect(plan.plan.preflight.review.budgets).toEqual({max_steps:32,max_seconds:180,max_output_tokens:1024,max_total_tokens:4096});
  expect(plan.plan.preflight.review.access_mode).toBe("read_only");expect(plan.plan.target.kind).toBe("document_engine");
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(input);expect(fetch).toHaveBeenCalledTimes(1);
});

it("rejects every raised or zero workflow limit even with a valid new plan checksum",async()=>{
  const input=decode("PrepareOwnerExecutionPlanV2",ocrBudget.prepare),limits={...defaultRunBudgets,...input.request.budgets};
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  for(const key of Object.keys(defaultRunBudgets) as (keyof typeof defaultRunBudgets)[]){
    for(const value of [0,limits[key]+1]){
      const plan=decode("PreparedExecutionPlanV2",structuredClone(ocrBudget.prepared));
      if(plan.plan.preflight.kind!=="owner")throw new Error("owner OCR fixture");
      plan.plan.preflight.review.budgets[key]=value;
      fetch.mockResolvedValueOnce(response(await rehash(plan)));
      await expect(client.prepareExecutionPlanV2(input)).rejects.toThrow("execution_plan_v2_response_mismatch");
    }
  }
  expect(fetch).toHaveBeenCalledTimes(8);expect(fetch.mock.calls.every(([url])=>!String(url).endsWith("/commit"))).toBe(true);
});

it("retains the explicit document engine, real user rights and same copy on commit",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(fixture.run));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner"),plan=await client.prepareExecutionPlanV2(request);
  expect(plan.plan.target.kind).toBe("document_engine");
  expect(plan.plan.boundary.process_access).toBe("user_rights");
  expect(plan.plan.boundary.process_network.kind).toBe("user_rights");
  expect(await client.commitExecutionPlanV2(plan)).toEqual(fixture.run);
  expect(JSON.parse(fetch.mock.calls[0][1].body).target).toEqual({kind:"document_engine"});
  expect(fetch.mock.calls).toHaveLength(2);
});

it("rejects forged isolation, a changed engine shape and general tools despite a new checksum",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  const changes:((p:PreparedExecutionPlanV2)=>void)[]=[
    p=>{p.plan.boundary.process_network={kind:"no_process"};},
    p=>{p.plan.boundary.process_network={kind:"denied"};},
    p=>{p.plan.boundary.process_access="broker_only";},
    p=>{if(p.plan.target.kind==="document_engine")p.plan.target.engine_revision="unbound";},
    p=>{if(p.plan.preflight.kind==="owner")p.plan.preflight.review.allowed_tools.push("execute_program");},
    p=>{if(p.plan.preflight.kind==="owner")p.plan.preflight.review.allowed_tools.push("browser");},
    p=>{p.plan.boundary.connector_aliases.push("private-account");},
  ];
  for(const change of changes){const p=prepared();change(p);fetch.mockResolvedValueOnce(response(await rehash(p)));
    await expect(client.prepareExecutionPlanV2(request)).rejects.toThrow("execution_plan_v2_integrity");}
  expect(fetch.mock.calls.every(([url])=>!String(url).endsWith("/commit"))).toBe(true);
});

it("never turns a process-free request into document execution or replaces its receipt target",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(response(prepared()));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.prepareExecutionPlanV2({...request,target:{kind:"broker_controller"}})).rejects.toThrow("execution_plan_v2_response_mismatch");
  const changed=prepared();if(changed.plan.target.kind!=="document_engine")throw new Error("document fixture");
  changed.plan.target.engine_revision="a".repeat(64);fetch.mockResolvedValueOnce(response(await rehash(changed)));
  await expect(client.inspectExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
});
