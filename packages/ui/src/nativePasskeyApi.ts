import {decode,type AndroidAuthTokens,type NativeAuthTransportView} from "@cowork/client";
export type NativePasskeyOutcome={operation_id:string,purpose:"native_token"|"registration_ack",response_json:string};
export function nativeOutcome(value:unknown):NativePasskeyOutcome{
  if(typeof value!=="object"||!value)throw new Error("native_auth_response_invalid");const v=value as NativePasskeyOutcome;
  if(!/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(v.operation_id)||!["native_token","registration_ack"].includes(v.purpose)||typeof v.response_json!=="string"||v.response_json.length>2048)throw new Error("native_auth_response_invalid");return v;
}
export function nativeTokens(value:NativePasskeyOutcome):AndroidAuthTokens&{schema_version:2}{
  if(value.purpose!=="native_token")throw new Error("native_auth_response_invalid");const result=decode("AndroidAuthTokens",JSON.parse(value.response_json));if(result.schema_version!==2)throw new Error("native_auth_response_invalid");return {...result,schema_version:2};
}
export function nativeRegistration(value:NativePasskeyOutcome):void{const result=JSON.parse(value.response_json);if(value.purpose!=="registration_ack"||result.registered!==true||typeof result.passkey_id!=="string"||!/^[0-9a-f-]{36}$/.test(result.passkey_id))throw new Error("native_auth_response_invalid");}
export function nativePending(value:unknown):NativeAuthTransportView[]{if(!Array.isArray(value)||value.length>64)throw new Error("native_auth_journal_mismatch");return value.map(v=>decode("NativeAuthTransportView",v));}
export async function confirmNative(value:NativePasskeyOutcome):Promise<void>{const {invoke}=await import("@tauri-apps/api/core");await invoke("native_passkey_resolve",{operationId:value.operation_id,acknowledged:true});}
export function nativeAuthError(value:unknown,language:string):string{const code=value instanceof Error?value.message:String(value),tr=(de:string,en:string)=>language==="de"?de:en;
  if(code.includes("native_auth_key_setup_required"))return tr("Richte zuerst die geschützte native Schlüsselablage ein. Sie bewahrt den ursprünglichen Vorgang bei einem App-Abbruch.","Set up the protected native key store first. It retains the original operation if the app is interrupted.");
  if(code.includes("native_auth_original_pending"))return tr("Ein ursprünglicher Vorgang ist noch offen. Wiederhole ihn unten oder entferne ausdrücklich seinen lokalen Beleg, bevor du einen neuen Dialog startest.","An original operation is still pending. Retry it below, or explicitly remove its local receipt before starting a new dialog.");
  if(code.includes("native_auth_original_session_changed")||code.includes("native_auth_original_session_required"))return tr("Die ursprünglich gebundene Sitzung ist nicht verfügbar. Der Vorgang wird nicht auf ein anderes Konto übertragen.","The originally bound session is unavailable. The operation cannot be transferred to another account.");
  return code;
}
