import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode} from "./index";

// Real Runtime/Store/Broker Single plan. Its public codec bytes convey no authority.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-document-single.json",import.meta.url),"utf8"));
const request=()=>decode("PrepareOwnerExecutionPlanV2",structuredClone(fixture.prepare));
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

it("retains the real Single document target, model and copy across the explicit confirmation",async()=>{
 const fetch=vi.fn().mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(fixture.run));vi.stubGlobal("fetch",fetch);
 const client=new Client("owner"),plan=await client.prepareExecutionPlanV2(request());
 expect(plan.plan.target.kind).toBe("document_engine");expect(plan.plan.boundary.process_access).toBe("user_rights");
 expect(await client.commitExecutionPlanV2(plan)).toEqual(fixture.run);
 expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request());
 expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({sha256:plan.sha256});
});

it("rejects a Single document result that loses the confirmed copy or substitutes its model",async()=>{
 const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
 for(const field of ["copy","model"]){
  const run=decode("RunRecord",structuredClone(fixture.run));
  if(field==="copy")delete run.spec.input_workspace;else if(run.spec.provider)run.spec.provider.id="unselected-model";
  fetch.mockResolvedValueOnce(response(run));
  await expect(client.commitExecutionPlanV2(prepared())).rejects.toThrow("execution_plan_v2_receipt_mismatch");
 }
});
