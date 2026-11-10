import {afterEach,expect,it,vi} from "vitest";
import {Client,type OwnerManagedProcessReconciliation,type ReconcileOwnerManagedProcess} from "./index";
import fixture from "./fixtures/owner-managed-reconciliation.json";

const original=fixture.response as OwnerManagedProcessReconciliation;
const intent=fixture.intent as ReconcileOwnerManagedProcess;
const order=original.run.spec.order_id,run=original.run.id,attempt=original.process_attempt.id;
const json=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();});

it("posts one explicit original intent using private transport and checks both scopes",async()=>{
  const fetch=vi.fn().mockResolvedValue(json(original));vi.stubGlobal("fetch",fetch);
  await expect(new Client("private-test-token","https://owner.invalid").reconcileOwnerManagedProcess(order,run,attempt,intent)).resolves.toEqual(original);
  expect(fetch).toHaveBeenCalledTimes(1);
  const [url,options]=fetch.mock.calls[0] as unknown as [string,RequestInit];
  expect(url).toBe(`https://owner.invalid/api/v2/orders/${order}/runs/${run}/managed-processes/${attempt}/reconcile`);
  expect(url).not.toContain("private-test-token");expect(options.headers).toMatchObject({Authorization:"Bearer private-test-token"});
  expect(options).toMatchObject({method:"POST",cache:"no-store",redirect:"error"});expect(JSON.parse(String(options.body))).toEqual(intent);
});
it("does not automatically retry a lost mutation response",async()=>{
  const fetch=vi.fn().mockRejectedValue(new TypeError("lost response"));vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").reconcileOwnerManagedProcess(order,run,attempt,intent)).rejects.toThrow("lost response");
  expect(fetch).toHaveBeenCalledTimes(1);
});
it("rejects malformed or renderer-authored evidence before transport",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  for(const bad of [{...intent,expected_run_revision:0},{...intent,expected_run_revision:Number.MAX_SAFE_INTEGER},
    {...intent,expected_request_sha256:"invalid"},{...intent,result:{termination:"canceled"}},
    {...intent,runner_signed_receipt:{}}]) {
    await expect(client.reconcileOwnerManagedProcess(order,run,attempt,bad)).rejects.toThrow();
  }
  expect(fetch).not.toHaveBeenCalled();
});
it.each(["order","run","attempt","request","revision","business","process","evidence"])("rejects a mismatched %s in a successful response",async changed=>{
  const response=structuredClone(original);
  if(changed==="order")response.run.spec.order_id+="-other";
  if(changed==="run")response.process_attempt.run_id+="-other";
  if(changed==="attempt")response.process_attempt.id+="-other";
  if(changed==="request")response.process_attempt.request_sha256="0".repeat(64);
  if(changed==="revision")response.run.revision++;
  if(changed==="business")response.run.state="completed";
  if(changed==="process")response.process_attempt.state={state:"launch_committed"};
  if(changed==="evidence"&&response.process_attempt.state.state==="reconciled_completed")response.process_attempt.state.evidence_sha256="invalid";
  vi.stubGlobal("fetch",vi.fn().mockResolvedValue(json(response)));
  await expect(new Client("owner").reconcileOwnerManagedProcess(order,run,attempt,intent)).rejects.toThrow("managed_process_reconciliation_integrity");
});
it("keeps a successful physical exit separate from business cancellation",async()=>{
  expect(original.run.state).toBe("canceled");
  expect(original.process_attempt.state).toMatchObject({state:"reconciled_completed",result:{termination:"exited",exit_code:0}});
  vi.stubGlobal("fetch",vi.fn().mockResolvedValue(json(original)));
  const result=await new Client("owner").reconcileOwnerManagedProcess(order,run,attempt,intent);
  expect(result.process_attempt.state).toEqual(original.process_attempt.state);
});
