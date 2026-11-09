import type {ModelRegistryRef} from "@cowork/client";
import {androidRequest} from "./androidDeviceApi";
import {PersistentRequestLatch,type RequestJournalStorage} from "./requestJournal";
import {validateActorRunConfiguration,type ActorAgentSelection} from "./ActorAgentConfigurationPanel";
import type {OutputContract} from "./StructuredOutputEditor";
import {githubUuid} from "./privateGithubApi";
export type ActorChatValue=ActorAgentSelection&{model_profile_id:string;provider_revision:number;model_binding:ModelRegistryRef|null;output:OutputContract|null};
export type ActorChatConfiguration={thread_id:string;project_id:string;revision:number;updated_at:number;value:ActorChatValue|null};
export type ActorChatWrite={expected_revision:number;idempotency_key:string;value:ActorChatValue};
function validValue(v:ActorChatValue):ActorChatValue{
  if(!v||!githubUuid(v.model_profile_id)||!Number.isSafeInteger(v.provider_revision)||v.provider_revision<1||Object.keys(v).some(k=>!["model_profile_id","provider_revision","run_configuration","skill_ids","memory_ids","mcp_metadata_ids","expected_mcp_revisions","model_binding","output"].includes(k)))throw new Error("private_chat_configuration_invalid");
  validateActorRunConfiguration(v.run_configuration);
  for(const [ids,max] of [[v.skill_ids,100],[v.memory_ids,100],[v.mcp_metadata_ids,16]] as const)if(!Array.isArray(ids)||ids.length>max||ids.some(id=>!githubUuid(id))||new Set(ids).size!==ids.length)throw new Error("private_chat_configuration_invalid");
  if(!v.expected_mcp_revisions||Object.entries(v.expected_mcp_revisions).some(([id,r])=>!v.mcp_metadata_ids.includes(id)||!Number.isSafeInteger(r)||r<1))throw new Error("private_chat_configuration_invalid");
  return structuredClone(v);
}
export class ActorChatConfigurationLatch extends PersistentRequestLatch<ActorChatWrite>{constructor(key?:string,storage?:RequestJournalStorage){super(key,raw=>{const v=raw as ActorChatWrite;if(!v||!githubUuid(v.idempotency_key)||!Number.isSafeInteger(v.expected_revision)||v.expected_revision<0||Object.keys(v).some(k=>!["idempotency_key","expected_revision","value"].includes(k)))throw new Error("private_chat_configuration_invalid");validValue(v.value);return v;},storage);}}
export async function actorChatConfiguration(base:string,session:string,project:string,thread:string,write?:ActorChatWrite):Promise<ActorChatConfiguration>{
  const value=await androidRequest<ActorChatConfiguration>(base,session,`/threads/${encodeURIComponent(thread)}/configuration`,write?"PUT":"GET",write);
  if(value.thread_id!==thread||value.project_id!==project||!Number.isSafeInteger(value.revision)||value.revision<0||value.value!==null&&!validValue(value.value))throw new Error("private_chat_configuration_invalid");
  if(write&&value.revision!==write.expected_revision+1)throw new Error("private_chat_configuration_invalid");
  return value;
}
