import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {chooseNativeFolder,listNativeFolders,revokeNativeFolder} from "./folderBridge";
const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
const input={project_id:"p",idempotency_key:"choice",writable:false,language:"de" as const};
const folder={grant:{id:"native:test",project_id:"p",name:"Chosen",writable:false,revision:"r",resource_id:"resource"},canonical_path:"C:\\fixture",revoked:false};
beforeEach(()=>{invoke.mockReset();vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});});
afterEach(()=>vi.unstubAllGlobals());

it("only sends project, rights and request identity; native code supplies the path and credentials",async()=>{
  invoke.mockResolvedValue({status:"registered",folder});
  expect(await chooseNativeFolder(input)).toEqual({status:"registered",folder});
  expect(invoke).toHaveBeenCalledExactlyOnceWith("choose_native_folder",{input});
  await expect(chooseNativeFolder({...input,path:"C:\\unapproved"} as typeof input)).rejects.toThrow("contract_mismatch");
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("does not give browsers a fallback registration mechanism and distinguishes picker cancellation",async()=>{
  vi.stubGlobal("window",{});
  await expect(chooseNativeFolder(input)).rejects.toMatchObject({details:{code:"folder_desktop_required"}});expect(invoke).not.toHaveBeenCalled();
  vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});invoke.mockResolvedValue({status:"canceled"});
  expect(await chooseNativeFolder(input)).toEqual({status:"canceled"});expect(invoke).toHaveBeenCalledTimes(1);
});
it("rejects foreign or elevated grant replies and never automatically repeats an uncertain request",async()=>{
  for(const reply of [{status:"registered",folder:{...folder,grant:{...folder.grant,project_id:"other"}}},{status:"registered",folder:{...folder,grant:{...folder.grant,writable:true}}},{}]) {
    invoke.mockResolvedValue(reply);await expect(chooseNativeFolder(input)).rejects.toMatchObject({details:{code:"folder_registration_uncertain",retryable:false}});
  }
  invoke.mockRejectedValue(new Error("IPC lost"));await expect(chooseNativeFolder(input)).rejects.toMatchObject({details:{code:"folder_registration_uncertain"}});expect(invoke).toHaveBeenCalledTimes(4);
  invoke.mockRejectedValue({code:"uncertain_effect",message:"Registry publication unconfirmed",retryable:false,correlation_id:"native-correlation"});
  await expect(chooseNativeFolder(input)).rejects.toMatchObject({details:{code:"folder_registration_uncertain",correlation_id:"native-correlation",retryable:false}});
  expect(invoke).toHaveBeenCalledTimes(5);
});
it("scopes listings and validates a revocation receipt against the exact grant revision",async()=>{
  invoke.mockResolvedValue([folder]);expect(await listNativeFolders("p")).toEqual([folder]);expect(invoke).toHaveBeenLastCalledWith("list_native_folders",{projectId:"p"});
  await expect(listNativeFolders("other")).rejects.toMatchObject({details:{code:"folder_connection_failed"}});
  const revoke={project_id:"p",folder_id:folder.grant.id,expected_revision:"r"};
  invoke.mockResolvedValue(folder.grant);expect(await revokeNativeFolder(revoke)).toEqual(folder.grant);
  invoke.mockResolvedValue({...folder.grant,revision:"newer"});await expect(revokeNativeFolder(revoke)).rejects.toMatchObject({details:{code:"folder_registration_uncertain"}});
});
