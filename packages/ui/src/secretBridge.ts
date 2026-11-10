import {decode,RequestError,type NativeSecretsStatus,type NativeSecretsSetup,type NativeSecretsReceipt,type NativeSecretsResolution,type UiLanguage} from "@cowork/client";

export const hasNativeSecrets=()=>typeof window!=="undefined"&&"__TAURI_INTERNALS__" in window;
const failure=(code:string)=>new RequestError({code,message:"Native key operation was not confirmed",retryable:false,correlation_id:crypto.randomUUID()},0);
async function call(command:string,args:Record<string,unknown>,mutation=false):Promise<unknown>{
  if(!hasNativeSecrets())throw failure("secrets_desktop_required");
  const {invoke}=await import("@tauri-apps/api/core");
  try{return await invoke(command,args);}
  catch(error){
    let details;try{details=decode("ApiError",error);}catch{throw failure(mutation?"secrets_setup_uncertain":"secrets_connection_failed");}
    if(mutation&&["uncertain_effect","storage_unavailable"].includes(details.code))details={...details,code:"secrets_setup_uncertain",retryable:false};
    throw new RequestError(details,0);
  }
}
function status(value:unknown):NativeSecretsStatus{
  const state=decode("NativeSecretsStatus",value);
  if(!Number.isSafeInteger(state.revision)||state.revision<0||(state.key_available&&!state.configured))throw failure("secrets_connection_failed");
  return state;
}
function receipt(value:unknown):NativeSecretsReceipt{
  const result=decode("NativeSecretsReceipt",value);status(result.store);return result;
}
export async function nativeSecretStatus(){
  try{return status(await call("native_secrets_status",{}));}
  catch(error){if(error instanceof RequestError)throw error;throw failure("secrets_connection_failed");}
}
export async function nativeSecretRequest(key:string){
  if(!key.trim()||key.length>128)throw failure("invalid_request");
  const lookup=decode("NativeSecretsRequestLookup",await call("native_secrets_request",{key}));
  return lookup.receipt===null?null:receipt(lookup.receipt);
}
export async function setupNativeSecrets(input:NativeSecretsSetup,language:UiLanguage){
  decode("NativeSecretsSetup",input);
  if(!Number.isSafeInteger(input.base_revision)||input.base_revision<0)throw failure("invalid_request");
  const value=await call("native_secrets_setup",{input,language},true);
  try{return value===null?null:receipt(value);}catch{throw failure("secrets_setup_uncertain");}
}
export async function resolveNativeSecrets(action:"reconcile"|"abandon",input:NativeSecretsResolution,language:UiLanguage){
  decode("NativeSecretsResolution",input);
  if(!Number.isSafeInteger(input.base_revision)||input.base_revision<=0||!["reconcile","abandon"].includes(action))throw failure("invalid_request");
  const value=await call(action==="reconcile"?"native_secrets_reconcile":"native_secrets_abandon",{input,language},true);
  if(value===null)return null;
  try{
    const result=receipt(value);
    if(result.operation_id!==input.operation_id||result.operation_state!==(action==="reconcile"?"activated":"abandoned"))throw new Error("operation");
    return result;
  }catch{throw failure("secrets_setup_uncertain");}
}
