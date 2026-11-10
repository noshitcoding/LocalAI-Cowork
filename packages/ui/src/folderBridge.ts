import {decode,RequestError,type NativeChooseFolderRequest,type RevokeFolder,type RegisteredFolder} from "@cowork/client";

export const hasNativeFolderManager=()=>typeof window!=="undefined"&&"__TAURI_INTERNALS__" in window;
const failure=(code:string)=>new RequestError({code,message:"Native folder result was not confirmed",retryable:false,correlation_id:crypto.randomUUID()},0);
async function call(command:string,args:Record<string,unknown>,mutation:boolean):Promise<unknown> {
  if(!hasNativeFolderManager())throw failure("folder_desktop_required");
  const {invoke}=await import("@tauri-apps/api/core");
  try{return await invoke(command,args);}
  catch(error){
    let details;try{details=decode("ApiError",error);}catch{throw failure(mutation?"folder_registration_uncertain":"folder_connection_failed");}
    // Registry publication may have succeeded before its readback/response failed.
    // Preserve the registration key for a lookup, rather than treating this as a file-reconciliation error.
    if(mutation&&details.code==="uncertain_effect")details={...details,code:"folder_registration_uncertain",retryable:false};
    throw new RequestError(details,0);
  }
}
export async function listNativeFolders(project:string):Promise<RegisteredFolder[]> {
  const value=await call("list_native_folders",{projectId:project},false);
  if(!Array.isArray(value))throw failure("folder_connection_failed");
  const records=value.map(row=>decode("RegisteredFolder",row));
  if(records.some(r=>r.grant.project_id!==project||r.grant.order_id))throw failure("folder_connection_failed");
  return records;
}
export async function chooseNativeFolder(input:NativeChooseFolderRequest){
  decode("NativeChooseFolderRequest",input);
  const value=await call("choose_native_folder",{input},true);
  try {
    const outcome=decode("NativeFolderOutcome",value);
    if(outcome.status==="registered"&&(outcome.folder.grant.project_id!==input.project_id||outcome.folder.grant.writable!==input.writable||outcome.folder.grant.order_id))throw new Error("scope");
    return outcome;
  }catch{throw failure("folder_registration_uncertain");}
}
export async function revokeNativeFolder(input:RevokeFolder){
  decode("RevokeFolder",input);
  const value=await call("revoke_native_folder",{input},true);
  try {
    const grant=decode("FolderGrant",value);
    if(grant.id!==input.folder_id||grant.project_id!==input.project_id||grant.revision!==input.expected_revision||grant.order_id)throw new Error("scope");
    return grant;
  }catch{throw failure("folder_registration_uncertain");}
}
