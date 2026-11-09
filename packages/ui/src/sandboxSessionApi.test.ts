import {describe,expect,it,vi,afterEach} from "vitest";
import {Client} from "@cowork/client";
import {SandboxSessionLatch,manageSandboxSession,inspectSandboxSession,sandboxExpectedUid,validateSandboxReceipt,validateSandboxRequest,validateSandboxPreview,recoverSandboxOperation,sandboxOperationHistory,definitiveSandboxRejection,type SandboxSessionRequest} from "./sandboxSessionApi";
const environment={environment_id:"env",version_id:"version",definition_sha256:"a".repeat(64),qualification_sha256:"b".repeat(64)};
const request:SandboxSessionRequest={environment,idempotency_key:"original-key",operation:"suspend",expected_sandbox_uid:"sandbox-original"};
const receipt={project_id:"project",order_id:"order",environment,operation_id:"durable-operation",sandbox_name:"sandbox",sandbox_uid:"sandbox-original",pod_uid:"pod-original",state:"suspended",workspace_preserved:true,observed_at:100};
afterEach(()=>vi.unstubAllGlobals());
describe("sandbox session authority and reconciliation",()=>{
  it("recovers unresolved requests beyond the hundred recent completed operations",async()=>{
    const values=[...Array.from({length:100},(_,index)=>({id:`completed-${index}`,state:"completed",request:{...request,idempotency_key:`completed-${index}`}})),{id:"unresolved",state:"dispatched",request}];
    const fetch=vi.fn().mockResolvedValue({ok:true,status:200,json:async()=>values});vi.stubGlobal("fetch",fetch);
    const client=new Client("secret","https://cowork.example");
    expect(await sandboxOperationHistory(client,"project","order")).toHaveLength(101);
    expect(await recoverSandboxOperation(client,"project","order",environment)).toEqual(request);
    expect(fetch.mock.calls.every(([,init])=>init.method==="GET"&&init.body===undefined)).toBe(true);
  });
  it("observes current identity with a separate read-only request before existing session changes",async()=>{
    const fetch=vi.fn().mockResolvedValue({ok:true,status:200,json:async()=>receipt});vi.stubGlobal("fetch",fetch);
    const observed=await inspectSandboxSession(new Client("secret","https://cowork.example"),"project","order",environment);
    const inspection=JSON.parse(fetch.mock.calls[0][1].body);
    expect(inspection).toMatchObject({operation:"inspect",expected_sandbox_uid:null,environment});
    expect(inspection.idempotency_key).not.toBe(request.idempotency_key);
    for(const operation of ["ensure","suspend","resume","stop"] as const){
      expect(()=>sandboxExpectedUid(operation,null,false)).toThrow("sandbox_session_inspection_required");
      expect(sandboxExpectedUid(operation,observed,true)).toBe("sandbox-original");
    }
    expect(sandboxExpectedUid("inspect",observed,true)).toBeNull();
  });
  it("permits fresh start only after definitive no-session observation and keeps unknown failures blocked",async()=>{
    const fetch=vi.fn().mockResolvedValueOnce({ok:false,status:404,json:async()=>({code:"sandbox_session_not_found"})});vi.stubGlobal("fetch",fetch);
    const client=new Client("secret","https://cowork.example");
    expect(await inspectSandboxSession(client,"project","order",environment)).toBeNull();
    expect(sandboxExpectedUid("ensure",null,true)).toBeNull();
    expect(sandboxExpectedUid("ensure",{...receipt,state:"stopped"},true)).toBeNull();
    for(const operation of ["suspend","resume","stop"] as const)expect(()=>sandboxExpectedUid(operation,null,true)).toThrow("sandbox_session_not_found");
    for(const [status,code] of [[404,"not_found"],[404,"request_failed"],[425,"workspace_busy"],[500,"sandbox_session_not_found"]] as const){
      fetch.mockResolvedValueOnce({ok:false,status,json:async()=>({code})});
      await expect(inspectSandboxSession(client,"project","order",environment)).rejects.toThrow(`${code} (${status})`);
    }
    fetch.mockResolvedValueOnce({ok:true,status:200,json:async()=>({...receipt,project_id:"other"})});
    await expect(inspectSandboxSession(client,"project","order",environment)).rejects.toThrow("sandbox_session_receipt_mismatch");
  });
  it("accepts deliberate Ensure UID rotation while retaining all receipt scope checks",()=>{
    const ensure={...request,operation:"ensure" as const};
    expect(validateSandboxReceipt({...receipt,sandbox_uid:"new-sandbox"},"project","order",ensure).sandbox_uid).toBe("new-sandbox");
    for(const operation of ["inspect","suspend","resume","stop"] as const)expect(()=>validateSandboxReceipt({...receipt,sandbox_uid:"new-sandbox"},"project","order",{...request,operation})).toThrow("sandbox_session_receipt_mismatch");
    for(const changed of [{project_id:"other"},{order_id:"other"},{environment:{...environment,version_id:"other"}}])expect(()=>validateSandboxReceipt({...receipt,...changed},"project","order",ensure)).toThrow("sandbox_session_receipt_mismatch");
  });
  it("clears authority only for an exact durably rejected or expired original request",()=>{
    for(const state of ["pending","dispatched","completed"]){expect(definitiveSandboxRejection([{id:"operation",state,request}],request)).toBe(false);}
    for(const state of ["rejected","expired"]){expect(definitiveSandboxRejection([{id:"operation",state,request}],request)).toBe(true);}
    expect(definitiveSandboxRejection([{id:"other",state:"rejected",request:{...request,idempotency_key:"different"}}],request)).toBe(false);
    expect(()=>definitiveSandboxRejection([{id:"changed",state:"rejected",request:{...request,operation:"stop"}}],request)).toThrow("sandbox_original_request_changed");
  });
  it("recovers an unresolved server request exactly without dispatching it",async()=>{
    const fetch=vi.fn().mockResolvedValue({ok:true,status:200,json:async()=>[{id:"operation",state:"dispatched",request}]});vi.stubGlobal("fetch",fetch);
    expect(await recoverSandboxOperation(new Client("secret","https://cowork.example"),"project","order",environment)).toEqual(request);
    expect(fetch).toHaveBeenCalledOnce();expect(fetch.mock.calls[0][1].method).toBe("GET");expect(fetch.mock.calls[0][1].body).toBeUndefined();
  });
  it("accepts preview tickets only on a separate HTTPS bootstrap origin without query credentials",()=>{
    const preview={url:"https://sandbox.previews.example.net/_cowork/bootstrap#signed_ticket",expires_at:1000};
    expect(validateSandboxPreview(preview,"https://app.example.com",1)).toEqual(preview);
    for(const url of ["https://app.example.com/_cowork/bootstrap#ticket","http://sandbox.previews.example.net/_cowork/bootstrap#ticket","https://sandbox.previews.example.net/?ticket=secret","https://sandbox.previews.example.net/_cowork/bootstrap?token=secret#ticket"]){expect(()=>validateSandboxPreview({...preview,url},"https://app.example.com",1)).toThrow();}
    expect(()=>validateSandboxPreview({...preview,expires_at:0},"https://app.example.com",1)).toThrow();
  });
  it("rejects injected infrastructure settings and substituted environment receipts",()=>{
    expect(()=>validateSandboxRequest({...request,namespace:"other"})).toThrow("sandbox_request_invalid");
    expect(()=>validateSandboxRequest({...request,environment:{...environment,host_path:"C:/"}})).toThrow("sandbox_environment_invalid");
    for(const changed of [{project_id:"other"},{order_id:"other"},{sandbox_uid:"recreated"},{environment:{...environment,version_id:"other"}}])expect(()=>validateSandboxReceipt({...receipt,...changed},"project","order",request)).toThrow("sandbox_session_receipt_mismatch");
    expect(validateSandboxReceipt(receipt,"project","order",request).operation_id).toBe("durable-operation");
  });
  it("retains the exact unresolved mutation across reload and separate status inspection",async()=>{
    const entries=new Map<string,string>(),storage={getItem:(key:string)=>entries.get(key)??null,setItem:(key:string,value:string)=>{entries.set(key,value);},removeItem:(key:string)=>{entries.delete(key);}};
    const latch=new SandboxSessionLatch("scope",storage),original=latch.prepare(()=>request);
    const fetch=vi.fn().mockRejectedValueOnce(new Error("lost acknowledgement"));vi.stubGlobal("fetch",fetch);
    const client=new Client("secret-token","https://cowork.example");
    await expect(manageSandboxSession(client,"project","order",original)).rejects.toThrow("lost acknowledgement");
    const restored=new SandboxSessionLatch("scope",storage);
    const exact=restored.prepare(()=>({...request,idempotency_key:"must-not-replace",operation:"stop"}));
    expect(exact).toEqual(request);expect(entries.get("scope")).not.toContain("secret-token");
    fetch.mockResolvedValueOnce({ok:true,json:async()=>receipt});
    await manageSandboxSession(client,"project","order",{...request,operation:"inspect",idempotency_key:"fresh-inspection"});
    expect(restored.pending).toBe(true);expect(restored.prepare(()=>{throw new Error();})).toEqual(request);
    fetch.mockResolvedValueOnce({ok:true,json:async()=>receipt});
    await manageSandboxSession(client,"project","order",exact);restored.confirm();expect(restored.pending).toBe(false);
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request);
    expect(JSON.parse(fetch.mock.calls[2][1].body)).toEqual(request);
    expect(fetch).toHaveBeenCalledTimes(3);
  });
});
