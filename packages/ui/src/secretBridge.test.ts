import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {nativeSecretStatus,nativeSecretRequest,setupNativeSecrets,resolveNativeSecrets} from "./secretBridge";
const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
const state={revision:2,configured:true,key_available:true,pending_operation_id:null};
const receipt={operation_id:"35c78a7d-6bc1-4b01-8a52-ff7b831f29b1",operation_state:"activated",store:state};
const setup={idempotency_key:"request",base_revision:0};
const resolution={operation_id:receipt.operation_id,base_revision:1};
beforeEach(()=>{invoke.mockReset();vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});});
afterEach(()=>vi.unstubAllGlobals());
it("never invokes native commands from a browser",async()=>{
  vi.stubGlobal("window",{});
  await expect(nativeSecretStatus()).rejects.toMatchObject({details:{code:"secrets_desktop_required"}});
  await expect(setupNativeSecrets(setup,"de")).rejects.toMatchObject({details:{code:"secrets_desktop_required"}});
  expect(invoke).not.toHaveBeenCalled();
});
it("passes only revision and request identity with language; no caller credentials or endpoint",async()=>{
  invoke.mockResolvedValue(receipt);
  expect(await setupNativeSecrets(setup,"en")).toEqual(receipt);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("native_secrets_setup",{input:setup,language:"en"});
  await expect(setupNativeSecrets({...setup,key:"secret"} as typeof setup,"de")).rejects.toThrow("contract_mismatch");
  await expect(setupNativeSecrets({...setup,base_revision:Number.MAX_SAFE_INTEGER+1},"de")).rejects.toMatchObject({details:{code:"invalid_request"}});
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("separates native cancellation from unknown publication and never retries",async()=>{
  invoke.mockResolvedValue(null);expect(await setupNativeSecrets(setup,"de")).toBeNull();
  invoke.mockRejectedValue(new Error("IPC lost"));
  await expect(setupNativeSecrets(setup,"de")).rejects.toMatchObject({details:{code:"secrets_setup_uncertain",retryable:false}});
  invoke.mockRejectedValue({code:"storage_unavailable",message:"Commit unconfirmed",retryable:false,correlation_id:"correlation"});
  await expect(setupNativeSecrets(setup,"de")).rejects.toMatchObject({details:{code:"secrets_setup_uncertain",correlation_id:"correlation"}});
  invoke.mockResolvedValue({});
  await expect(setupNativeSecrets(setup,"de")).rejects.toMatchObject({details:{code:"secrets_setup_uncertain"}});
  expect(invoke).toHaveBeenCalledTimes(4);
});
it("binds reconciliation and abandonment receipts to the exact operation and outcome",async()=>{
  invoke.mockResolvedValue({...receipt,operation_id:"other"});
  await expect(resolveNativeSecrets("reconcile",resolution,"de")).rejects.toMatchObject({details:{code:"secrets_setup_uncertain"}});
  invoke.mockResolvedValue(receipt);
  await expect(resolveNativeSecrets("abandon",resolution,"de")).rejects.toMatchObject({details:{code:"secrets_setup_uncertain"}});
  invoke.mockResolvedValue({...receipt,operation_state:"abandoned"});
  expect((await resolveNativeSecrets("abandon",resolution,"en"))?.operation_state).toBe("abandoned");
  expect(invoke).toHaveBeenLastCalledWith("native_secrets_abandon",{input:resolution,language:"en"});
});
it("reads setup receipts without mutation and rejects inconsistent status or secret-bearing responses",async()=>{
  invoke.mockResolvedValue({receipt});expect(await nativeSecretRequest("request")).toEqual(receipt);
  expect(invoke).toHaveBeenLastCalledWith("native_secrets_request",{key:"request"});
  for(const wrong of [{...state,configured:false},{...state,revision:-1},{...state,key:"must not enter UI"}]){
    invoke.mockResolvedValue(wrong);
    await expect(nativeSecretStatus()).rejects.toThrow();
  }
  invoke.mockResolvedValue({...state,key_available:false});
  expect(await nativeSecretStatus()).toEqual({...state,key_available:false});
  expect(invoke.mock.calls.every(([name])=>["native_secrets_request","native_secrets_status"].includes(name))).toBe(true);
});
