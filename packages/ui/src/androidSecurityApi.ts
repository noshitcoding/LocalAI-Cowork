import {decode,type AndroidAuthenticationAssurance,type AndroidTotpStatus,type AndroidTotpSetup,type AndroidRecoveryCodes,type AndroidInvitationSecret,type AndroidAuthSession,type AndroidAuthTokens} from "@cowork/client";
export type Assurance=AndroidAuthenticationAssurance;
export type TotpStatus=AndroidTotpStatus;
export type TotpSetup=AndroidTotpSetup;
export type RecoveryCodes=AndroidRecoveryCodes;
export type Invitation=AndroidInvitationSecret;
export type SecuritySession=AndroidAuthSession;
export function browserSession(endpoint:string):boolean {if("__TAURI_INTERNALS__" in window||location.protocol!=="https:")return false;try{return new URL(endpoint).origin===location.origin;}catch{return false;}}
export async function configuredBrowserSession(endpoint:string):Promise<boolean>{if(!browserSession(endpoint))return false;const response=await fetch(endpoint+"/api/v1/auth/passkeys/config",{cache:"no-store",redirect:"error",credentials:"omit"});if(!response.ok)return false;const config=await response.json() as {origin?:unknown};return config.origin===location.origin;}
/** Keys and request bytes survive a live lost acknowledgement. Secrets stay in
 * memory; no credential or returned token is written to browser persistence. */
export async function securityRequest<T>(endpoint:string,token:string,path:string,method="GET",body?:unknown,operation?:string):Promise<T> {
  const browser=["/auth/login","/auth/invitations/accept","/auth/browser/refresh","/auth/logout"].includes(path)&&await configuredBrowserSession(endpoint);
  if(path==="/auth/browser/refresh"&&!browser)throw new Error("browser_session_origin_not_configured");
  const response=await fetch(endpoint+"/api/v1"+path,{method,headers:{...(token?{Authorization:"Bearer "+token}:{}),...(body!==undefined?{"Content-Type":"application/json"}:{}),...(operation?{"Idempotency-Key":operation}:{}),...(browser?{"x-cowork-session-mode":"browser-cookie"}:{})},body:body===undefined?undefined:JSON.stringify(body),credentials:browser?"same-origin":"omit",cache:"no-store",redirect:"error"});
  if(!response.ok){let code="security_request_failed";try{const value=await response.json();code=String(value.error??code);}catch{}throw new Error(`${code} (${response.status})`);}
  if(response.status===204)return undefined as T;
  const value:unknown=await response.json();let result:unknown;
  if(path==="/auth/login"||path==="/auth/invitations/accept"||path==="/auth/browser/refresh")result=decode(browser?"AndroidBrowserAuthTokens":"AndroidAuthTokens",value);
  else if(path==="/auth/assurance"||path==="/auth/assurance/verify")result=decode("AndroidAuthenticationAssurance",value);
  else if(path==="/auth/security-context")result=decode("AndroidAuthSecurityContext",value);
  else if(path==="/auth/logout")return undefined as T;
  else if(path==="/auth/totp")result=decode("AndroidTotpStatus",value);
  else if(path==="/auth/totp/setup")result=decode("AndroidTotpSetup",value);
  else if(path==="/auth/totp/enable"||path==="/auth/totp/recovery-codes")result=decode("AndroidRecoveryCodes",value);
  else if(path==="/auth/invitations")result=decode("AndroidInvitationSecret",value);
  else if(path==="/auth/sessions"){if(!Array.isArray(value))throw new Error("auth_contract_mismatch");result=value.map(v=>decode("AndroidAuthSession",v));}
  else throw new Error("auth_contract_mismatch");
  const rows=Array.isArray(result)?result:[result];
  for(const row of rows)if(typeof row==="object"&&row&&"schema_version"in row&&row.schema_version!==2)throw new Error("auth_contract_mismatch");
  return result as T;
}
export function securityDevice(endpoint:string):string {
  const key="cowork.auth.device.v1:"+endpoint;
  const value=localStorage.getItem(key);if(value&&/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(value)&&value!=="00000000-0000-0000-0000-000000000000")return value;
  const id=crypto.randomUUID();localStorage.setItem(key,id);return id;
}
export type SecurityTokens=Omit<AndroidAuthTokens,"refresh_token">&{schema_version:2,refresh_token?:string};
const operationStorage=(endpoint:string,purpose:string,binding:string)=>"cowork.auth.pending.v1:"+JSON.stringify([endpoint,purpose,binding]);
export function securityOperation(endpoint:string,purpose:string,binding:string):string {
  const key=operationStorage(endpoint,purpose,binding),old=localStorage.getItem(key);
  if(old){if(!/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(old)||old==="00000000-0000-0000-0000-000000000000")throw new Error("auth_receipt_invalid");return old;}
  const id=crypto.randomUUID();localStorage.setItem(key,id);return id;
}
export function securityOperationConfirmed(endpoint:string,purpose:string,binding:string):void {localStorage.removeItem(operationStorage(endpoint,purpose,binding));}
export async function invitationBinding(token:string):Promise<string> {
  // The invitation token has 256 random bits. Its digest binds a nonsecret
  // operation locator; no password or low-entropy OTP digest is persisted.
  const bytes=await crypto.subtle.digest("SHA-256",new TextEncoder().encode(token));return Array.from(new Uint8Array(bytes),b=>b.toString(16).padStart(2,"0")).join("");
}
