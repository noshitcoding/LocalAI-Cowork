import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,decode,type PreparedExecutionPlan} from "./index";

const request={order_id:"order",base_revision:1,idempotency_key:"original",prompt:"Review",provider_id:"provider",provider_revision:1};
const provider={id:"provider",name:"Model",kind:"test" as const,endpoint:"",model:"fixture",revision:1,context_tokens:8192,credential_required:false,credential_ready:true};
const budgets={max_steps:64,max_seconds:1800,max_output_tokens:4096,max_total_tokens:262144};
function preparation():PreparedExecutionPlan{return decode("PreparedExecutionPlan",{
  plan:{format_version:1,id:"plan",actor:"installation-owner",project_id:"project",order_id:"order",idempotency_key:"original",
    request_sha256:"b".repeat(64),spec_sha256:"c".repeat(64),created_at:1000,expires_at:301000,
    preflight:{ready:true,provider:structuredClone(provider),executor:"local-v2",policy_revision:1,access_mode:"confirm_mutations",allowed_tools:["read_artifact"],budgets},
    boundary:{executor:"local-v2",process_access:"broker_only",process_network:{kind:"no_process"},environment:null,folders:[],connector_aliases:[],input_snapshot_bound:false}},
  sha256:"a".repeat(64),committed_run_id:null,retired:false,
});}
const run={id:"run",spec:{order_id:"order",project_id:"project",prompt:"Review",provider,access_mode:"confirm_mutations",executor:"local-v2",policy_revision:1,allowed_tools:["read_artifact"],budgets},state:"queued",revision:1,last_sequence:1,steps:0,created_at:1000,updated_at:1000,usage:{input_tokens:null,output_tokens:null,cost:null,price_source_version:null}};
const response=(body:unknown)=>new Response(JSON.stringify(body),{status:200});
afterEach(()=>vi.unstubAllGlobals());

describe("reviewed execution transport",()=>{
  it("prepares without committing and rejects substituted actor, request, model and native authority",async()=>{
    const fetch=vi.fn().mockImplementation(()=>Promise.resolve(response(preparation())));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner","https://cowork.example");
    await expect(client.prepareExecutionPlan(request)).resolves.toMatchObject({sha256:"a".repeat(64)});
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0][0]).toBe("https://cowork.example/api/v2/execution-plans");
    for(const mutate of [
      (value:PreparedExecutionPlan)=>{value.plan.actor="remote-actor";},
      (value:PreparedExecutionPlan)=>{value.plan.idempotency_key="substitution";},
      (value:PreparedExecutionPlan)=>{value.plan.order_id="another-order";},
      (value:PreparedExecutionPlan)=>{if("provider" in value.plan.preflight)value.plan.preflight.provider.id="fallback";},
      (value:PreparedExecutionPlan)=>{value.plan.boundary.process_access="user_rights";},
    ]){
      const value=preparation();mutate(value);fetch.mockImplementation(()=>Promise.resolve(response(value)));
      await expect(client.prepareExecutionPlan(request)).rejects.toThrow("execution_plan_response_mismatch");
    }
    expect(fetch.mock.calls.every(call=>!String(call[0]).endsWith("/commit"))).toBe(true);
  });
  it("confirms only the reviewed plan hash and binds the returned run",async()=>{
    const fetch=vi.fn().mockImplementation(()=>Promise.resolve(response(run)));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner");
    await expect(client.commitExecutionPlan(preparation())).resolves.toMatchObject({id:"run"});
    expect(fetch.mock.calls[0][0]).toBe("/api/v2/orders/order/execution-plans/plan/commit");
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({sha256:"a".repeat(64)});
    fetch.mockImplementation(()=>Promise.resolve(response({...run,spec:{...run.spec,executor:"another-host"}})));
    await expect(client.commitExecutionPlan(preparation())).rejects.toThrow("execution_plan_receipt_mismatch");
    fetch.mockImplementation(()=>Promise.resolve(response({...run,spec:{...run.spec,allowed_tools:["execute_program"]}})));
    await expect(client.commitExecutionPlan(preparation())).rejects.toThrow("execution_plan_receipt_mismatch");
  });
  it("never automatically retries a lost commit and keeps explicit retries identical",async()=>{
    const fetch=vi.fn().mockRejectedValue(new TypeError("lost acknowledgement"));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner"),original=preparation();
    await expect(client.commitExecutionPlan(original)).rejects.toThrow("lost acknowledgement");
    expect(fetch).toHaveBeenCalledTimes(1);
    await expect(client.commitExecutionPlan(original)).rejects.toThrow("lost acknowledgement");
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls[0]).toEqual(fetch.mock.calls[1]);
  });
  it("routes convenience starts through prepare and exact confirmation",async()=>{
    const fetch=vi.fn().mockImplementation((url:string)=>Promise.resolve(response(url.endsWith("/execution-plans")?preparation():run)));vi.stubGlobal("fetch",fetch);
    await expect(new Client("owner").createRun(request)).resolves.toMatchObject({id:"run"});
    expect(fetch.mock.calls.map(call=>call[0])).toEqual(["/api/v2/execution-plans","/api/v2/orders/order/execution-plans/plan/commit"]);
  });
  it("does not accept a retirement response that already references execution",async()=>{
    const fetch=vi.fn().mockImplementation(()=>Promise.resolve(response({...preparation(),retired:true,committed_run_id:"run"})));vi.stubGlobal("fetch",fetch);
    await expect(new Client("owner").retireExecutionPlan(preparation())).rejects.toThrow("execution_plan_retirement_mismatch");
  });
});
