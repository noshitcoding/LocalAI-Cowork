import {afterEach, describe, expect, it, vi} from "vitest";
import {Client, decode} from "./index";
import {defaultRunBudgets} from "./generated";
const request={kind:"workflow" as const,order_id:"order",base_revision:1,idempotency_key:"same",
  prompt:"Review",workflow_id:"workflow",version_id:"version"};
const binding={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};
describe("public workflow contracts",()=>{
  afterEach(()=>vi.unstubAllGlobals());
  it("accepts the explicit workflow request and rejects client authority or mixed starts",()=>{
    expect(decode("RunRequest",request)).toEqual(request);
    expect(decode("RunRequest",{...request,folders:[],mcp_tools:[]})).toEqual({...request,folders:[],mcp_tools:[]});
    for(const extra of [{max_parallel:999},{provider_id:"other"},
      {grant:"forged"},{plan:{}},{spec:{}},{kind:"delegated"}]) {
      expect(()=>decode("RunRequest",{...request,...extra})).toThrow("contract_mismatch");
    }
    const legacy={order_id:"order",base_revision:1,idempotency_key:"single",prompt:"Read",provider_id:"profile",provider_revision:1};
    expect(decode("RunRequest",legacy)).toEqual(legacy);
  });
  it("keeps version binding and confirmed workflow preflight distinct from a single model",()=>{
    const flight={ready:true,workflow:binding,name:"Create and review",version_revision:2,
      executor:"local-v2",policy_revision:1,access_mode:"confirm_mutations",budgets:defaultRunBudgets,
      allowed_tools:["create_artifact"],roles:[],max_parallel:2};
    expect(decode("RunPreflight",flight)).toEqual(flight);
    expect(()=>decode("RunPreflight",{...flight,provider_id:"fallback"})).toThrow("contract_mismatch");
    expect(()=>decode("WorkflowBinding",{...binding,grant:"unexpected"})).toThrow("contract_mismatch");
  });
  it("never retries a failed start and preserves the exact request when the user retries",async()=>{
    const fetch=vi.fn().mockRejectedValue(new TypeError("lost acknowledgement"));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner");const frozen={...request,expected_binding:binding};
    await expect(client.createRun(frozen)).rejects.toThrow("lost acknowledgement");
    expect(fetch).toHaveBeenCalledTimes(1);
    await expect(client.createRun(frozen)).rejects.toThrow("lost acknowledgement");
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls.map(call=>JSON.parse(call[1].body))).toEqual([frozen,frozen]);
    expect(fetch.mock.calls[0][1].headers.Authorization).toBe("Bearer owner");
  });
  it("scopes library pagination to encoded project paths",async()=>{
    const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify({items:[],total:0,next_offset:null}),{status:200}));
    vi.stubGlobal("fetch",fetch);await expect(new Client("owner").workflows("a/b",30,10)).resolves.toEqual({items:[],total:0,next_offset:null});
    expect(fetch.mock.calls[0][0]).toBe("/api/v2/projects/a%2Fb/workflows?offset=30&limit=10");
  });
  it("rejects malformed action ownership before presenting an approval",()=>{
    const approval={id:"approval",run_id:"run",tool:{id:"call",name:"create_artifact",arguments:{name:"Result",content:"data"}},
      fingerprint:"f".repeat(64),target:"local-v2",policy_revision:1,decision:null};
    expect(decode("WorkflowRunApproval",{approval,node_id:"step",role_id:"author"})).toEqual({approval,node_id:"step",role_id:"author"});
    expect(decode("WorkflowRunApproval",{approval,node_id:"step"})).toEqual({approval,node_id:"step"});
    expect(()=>decode("WorkflowRunApproval",{approval,role_id:"author"})).toThrow("contract_mismatch");
    expect(()=>decode("WorkflowRunApproval",{approval,node_id:"step",role_id:7})).toThrow("contract_mismatch");
    expect(()=>decode("WorkflowRunApproval",{approval,node_id:"step",role_id:"author",grant:"unexpected"})).toThrow("contract_mismatch");
  });
});
