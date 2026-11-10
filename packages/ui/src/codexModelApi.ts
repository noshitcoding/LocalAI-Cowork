import {decode,type Client,type CodexAccount,type CodexModel,type ProviderProfile} from "@cowork/client";
import {PersistentRequestLatch,requestJournalEndpoint} from "./requestJournal";
import type {AgentModelSelection,AgentTemplate,Personality} from "./libraryApi";

export type CodexContext={client:Client;session?:string;actorId?:string};
type Selection=Extract<AgentModelSelection,{kind:"codex"}>;
type Pending={account_id:string;requested_model:string|null;requested_effort:string|null;body:{idempotency_key:string;expected_account_revision:number;model:string;reasoning_effort:string|null;context_tokens:number}};
const identifier=(v:unknown)=>typeof v==="string"&&/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(v);
class CodexModelError extends Error{constructor(readonly status:number,code:string){super(code);}}
function validate(raw:unknown):Pending{
  const v=raw as Pending,b=v?.body;
  if(!v||!identifier(v.account_id)||!b||!identifier(b.idempotency_key)||!Number.isSafeInteger(b.expected_account_revision)||b.expected_account_revision<1||typeof b.model!=="string"||!b.model||b.model.length>200||!(b.reasoning_effort===null||typeof b.reasoning_effort==="string"&&/^[a-zA-Z0-9_-]{1,32}$/.test(b.reasoning_effort))||!Number.isSafeInteger(b.context_tokens)||b.context_tokens<1024||b.context_tokens>2000000||!(v.requested_model===null||typeof v.requested_model==="string")||!(v.requested_effort===null||typeof v.requested_effort==="string")||Object.keys(v).some(k=>!["account_id","requested_model","requested_effort","body"].includes(k))||Object.keys(b).some(k=>!["idempotency_key","expected_account_revision","model","reasoning_effort","context_tokens"].includes(k)))throw new Error("codex_profile_reference_invalid");
  return v;
}
function journal(context:CodexContext,account:string){
  if(!identifier(account)||context.session!==undefined&&(!context.session||!context.actorId)||context.session===undefined&&!context.client.token)throw new Error("codex_profile_session_required");
  return new PersistentRequestLatch<Pending>("cowork.codex-model-profile.v1:"+JSON.stringify([requestJournalEndpoint(context.client.base),context.session!==undefined?context.actorId:"installation-owner",account]),validate);
}
export async function codexRequest<T>(context:CodexContext,path:string,method="GET",body?:unknown):Promise<T>{
  if(context.session!==undefined&&!context.session)throw new Error("codex_profile_session_required");
  const response=await fetch(context.client.base+(context.session!==undefined?"/api/v1":"/api/v2")+"/codex"+path,{method,headers:{Authorization:"Bearer "+(context.session??context.client.token),...(body===undefined?{}:{"Content-Type":"application/json"})},body:body===undefined?undefined:JSON.stringify(body),credentials:"omit",redirect:"error",cache:"no-store"});
  if(!response.ok){let code="codex_profile_request_failed";try{const value=await response.json();code=String(value.code??value.error??code);}catch{}throw new CodexModelError(response.status,code);}
  return response.json() as Promise<T>;
}
async function submit(context:CodexContext,latch:PersistentRequestLatch<Pending>,original:Pending):Promise<ProviderProfile>{
    const profile=decode("ProviderProfile",await codexRequest(context,`/accounts/${encodeURIComponent(original.account_id)}/model-profile`,"POST",original.body));
    const binding=new URL(profile.endpoint);
    const expectedEndpoint=`codex://${original.account_id}/${original.body.expected_account_revision}${original.body.reasoning_effort?"?reasoning_effort="+original.body.reasoning_effort:""}`;
    if(profile.endpoint!==expectedEndpoint||!identifier(profile.id)||String(profile.kind)!=="codex"||profile.model!==original.body.model||profile.context_tokens!==original.body.context_tokens||binding.protocol!=="codex:"||binding.hostname!==original.account_id||binding.pathname!=="/"+original.body.expected_account_revision||(binding.searchParams.get("reasoning_effort")??null)!==original.body.reasoning_effort)throw new Error("codex_profile_receipt_mismatch");
    // Only a matching acknowledgement settles the original request. Even a 4xx
    // on a retry can follow an earlier commit whose response was lost.
    latch.confirm();return profile;
}
/** The journal contains model metadata only. A lost response reuses its original account revision. */
export async function materializeCodexModelProfile(context:CodexContext,selection:Selection,contextTokens=32768):Promise<ProviderProfile>{
  const latch=journal(context,selection.account_id);
  if(latch.pending){const original=latch.prepare(()=>{throw new Error("codex_profile_reference_invalid");});if(original.requested_model!==selection.model||original.requested_effort!==selection.reasoning_effort||original.body.context_tokens!==contextTokens)throw new Error("codex_profile_reconcile_original_first");return submit(context,latch,original);}
  const accounts=await codexRequest<unknown[]>(context,"/accounts");
  if(!Array.isArray(accounts))throw new Error("codex_accounts_invalid");
  const account=accounts.map(value=>decode("CodexAccount",value)).find(value=>value.id===selection.account_id) as CodexAccount|undefined;
  if(!account)throw new Error("codex_account_unavailable");
  const raw=await codexRequest<unknown[]>(context,`/accounts/${encodeURIComponent(account.id)}/models`);
  if(!Array.isArray(raw))throw new Error("codex_models_invalid");
  const models=raw.map(value=>decode("CodexModel",value)),model:CodexModel|undefined=selection.model?models.find(value=>value.id===selection.model):models[0];
  if(!model)throw new Error("codex_model_unavailable");
  const effort=selection.reasoning_effort??model.default_reasoning_effort??null;
  if(effort!==null&&!model.reasoning_efforts.includes(effort))throw new Error("codex_reasoning_effort_unavailable");
  const original=latch.prepare(()=>({account_id:account.id,requested_model:selection.model,requested_effort:selection.reasoning_effort,body:{idempotency_key:crypto.randomUUID(),expected_account_revision:account.revision,model:model.id,reasoning_effort:effort,context_tokens:contextTokens}}));
  return submit(context,latch,original);
}
export const pendingCodexModelProfile=(context:CodexContext,account:string)=>journal(context,account).pending;
export async function recoverCodexModelProfile(context:CodexContext,account:string){const latch=journal(context,account);return submit(context,latch,latch.prepare(()=>{throw new Error("codex_profile_reference_missing");}));}
export function discardCodexModelProfile(context:CodexContext,account:string){journal(context,account).discard();}
/** Resolve a Codex choice in an independent copy; the saved template and its revision remain intact. */
export async function bindCodexLibraryModel<T extends AgentTemplate|Personality>(context:CodexContext,item:T):Promise<{item:T;profile?:ProviderProfile}>{
  const value=item.value,model:AgentModelSelection="configuration"in value?(value.configuration.model.kind==="inherit"?value.personality?.value.model??{kind:"inherit"}:value.configuration.model):value.model;
  if(model.kind!=="codex")return {item};
  const profile=await materializeCodexModelProfile(context,model),copy=structuredClone(item),selection:AgentModelSelection={kind:"provider",provider_id:profile.id,provider_revision:profile.revision};
  if("configuration"in copy.value)copy.value.configuration.model=selection;else copy.value.model=selection;
  return {item:copy,profile};
}
