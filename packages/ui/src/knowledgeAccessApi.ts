import {RequestError,type Client} from "@cowork/client";
import {productRequest} from "./productApi";
import type {MemoryMetadata} from "./memoryMetadata";

/** Local owner switches: they only narrow which knowledge new runs receive. */
export type OwnerKnowledgeAccess={revision:number;personal_knowledge:boolean};
export type ProjectKnowledgeAccess={project_id:string;revision:number;project_knowledge:boolean;personal_knowledge:boolean;personal_available:boolean};
/** Server administrator policy (platform administrators only). The personal-knowledge-in-team-runs
 *  fields are undefined when the server predates that switch; they are then never sent back. */
export type KnowledgeAccessPolicy={revision:number;private_projects:boolean;team_projects:boolean;enabled_users:string[];enabled_projects:string[];updated_at:number|null;updated_by:string|null;personal_team_runs?:boolean;enabled_personal_team_users?:string[]};
export type KnowledgeAccessUser={id:string;username:string;display_name:string;active:boolean;private_projects:boolean;personal_team_runs?:boolean};
export type KnowledgeAccessProject={id:string;name:string;team_id:string;team_name:string;archived:boolean;team_projects:boolean};
export type KnowledgeAccessDirectory={policy:KnowledgeAccessPolicy;users:KnowledgeAccessUser[];projects:KnowledgeAccessProject[];truncated:boolean};
export type WriteKnowledgeAccessPolicy={expected_revision:number;private_projects:boolean;team_projects:boolean;enabled_users:string[];enabled_projects:string[];personal_team_runs?:boolean;enabled_personal_team_users?:string[]};
export type ActorKnowledgeAccess={private_projects:boolean;personal_team_runs:boolean};

/** Personal knowledge entries are user-scoped memories kept in the always present chat space. */
export const personalKnowledgeProject="__cowork_independent_chats__";
export const personalMemory=():MemoryMetadata=>({scope:{kind:"user"},category:"knowledge",confidence:null,profile_key:null,enabled:true});
export const isPersonalEntry=(entry:{memory?:MemoryMetadata|null})=>entry.memory?.scope.kind==="user";

const object=(raw:unknown):Record<string,unknown>=>{if(!raw||typeof raw!=="object"||Array.isArray(raw))throw new Error("knowledge_access_invalid");return raw as Record<string,unknown>;};
const keys=(v:Record<string,unknown>,allowed:string[],optional:string[]=[])=>{if(Object.keys(v).some(k=>!allowed.includes(k)&&!optional.includes(k))||allowed.some(k=>!(k in v)))throw new Error("knowledge_access_invalid");};
const revision=(v:unknown)=>{if(!Number.isSafeInteger(v)||(v as number)<0)throw new Error("knowledge_access_invalid");return v as number;};
const bool=(v:unknown)=>{if(typeof v!=="boolean")throw new Error("knowledge_access_invalid");return v;};
const text=(v:unknown)=>{if(typeof v!=="string")throw new Error("knowledge_access_invalid");return v;};
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const ids=(v:unknown)=>{if(!Array.isArray(v)||v.some(id=>typeof id!=="string"||!uuid.test(id))||new Set(v).size!==v.length)throw new Error("knowledge_access_invalid");return v as string[];};

export function validateOwnerAccess(raw:unknown):OwnerKnowledgeAccess{const v=object(raw);keys(v,["revision","personal_knowledge"]);return {revision:revision(v.revision),personal_knowledge:bool(v.personal_knowledge)};}
export function validateProjectAccess(raw:unknown,project:string):ProjectKnowledgeAccess{
  const v=object(raw);keys(v,["project_id","revision","project_knowledge","personal_knowledge","personal_available"]);
  if(v.project_id!==project)throw new Error("knowledge_access_invalid");
  return {project_id:project,revision:revision(v.revision),project_knowledge:bool(v.project_knowledge),personal_knowledge:bool(v.personal_knowledge),personal_available:bool(v.personal_available)};
}
export function validatePolicy(raw:unknown):KnowledgeAccessPolicy{
  const v=object(raw);keys(v,["revision","private_projects","team_projects","enabled_users","enabled_projects","updated_at","updated_by"],["personal_team_runs","enabled_personal_team_users"]);
  if(!(v.updated_at===null||Number.isSafeInteger(v.updated_at))||!(v.updated_by===null||typeof v.updated_by==="string"&&uuid.test(v.updated_by)))throw new Error("knowledge_access_invalid");
  if(("personal_team_runs" in v)!==("enabled_personal_team_users" in v))throw new Error("knowledge_access_invalid");
  const policy:KnowledgeAccessPolicy={revision:revision(v.revision),private_projects:bool(v.private_projects),team_projects:bool(v.team_projects),enabled_users:ids(v.enabled_users),enabled_projects:ids(v.enabled_projects),updated_at:v.updated_at as number|null,updated_by:v.updated_by as string|null};
  if("personal_team_runs" in v){policy.personal_team_runs=bool(v.personal_team_runs);policy.enabled_personal_team_users=ids(v.enabled_personal_team_users);}
  return policy;
}
/** Older servers report only private_projects; they never allow personal knowledge in team runs. */
export function validateActorAccess(raw:unknown):ActorKnowledgeAccess{const v=object(raw);keys(v,["private_projects"],["personal_team_runs"]);return {private_projects:bool(v.private_projects),personal_team_runs:"personal_team_runs" in v?bool(v.personal_team_runs):false};}
export function validateDirectory(raw:unknown):KnowledgeAccessDirectory{
  const v=object(raw);keys(v,["policy","users","projects","truncated"]);
  if(!Array.isArray(v.users)||!Array.isArray(v.projects))throw new Error("knowledge_access_invalid");
  const users=v.users.map(item=>{const u=object(item);keys(u,["id","username","display_name","active","private_projects"],["personal_team_runs"]);if(!uuid.test(text(u.id)))throw new Error("knowledge_access_invalid");const user:KnowledgeAccessUser={id:u.id as string,username:text(u.username),display_name:text(u.display_name),active:bool(u.active),private_projects:bool(u.private_projects)};if("personal_team_runs" in u)user.personal_team_runs=bool(u.personal_team_runs);return user;});
  const projects=v.projects.map(item=>{const p=object(item);keys(p,["id","name","team_id","team_name","archived","team_projects"]);if(!uuid.test(text(p.id))||!uuid.test(text(p.team_id)))throw new Error("knowledge_access_invalid");return {id:p.id as string,name:text(p.name),team_id:p.team_id as string,team_name:text(p.team_name),archived:bool(p.archived),team_projects:bool(p.team_projects)};});
  return {policy:validatePolicy(v.policy),users,projects,truncated:bool(v.truncated)};
}

export async function ownerKnowledgeAccess(client:Client){return validateOwnerAccess(await productRequest<unknown>(client,"/knowledge-access"));}
export async function writeOwnerKnowledgeAccess(client:Client,body:{expected_revision:number;personal_knowledge:boolean}){return validateOwnerAccess(await productRequest<unknown>(client,"/knowledge-access","PUT",body));}
export async function projectKnowledgeAccess(client:Client,project:string){return validateProjectAccess(await productRequest<unknown>(client,`/projects/${encodeURIComponent(project)}/knowledge-access`),project);}
export async function writeProjectKnowledgeAccess(client:Client,project:string,body:{expected_revision:number;project_knowledge:boolean;personal_knowledge:boolean}){return validateProjectAccess(await productRequest<unknown>(client,`/projects/${encodeURIComponent(project)}/knowledge-access`,"PUT",body),project);}

export async function knowledgeAccessDirectory(client:Client,session:string){return validateDirectory(await productRequest<unknown>(client,"/teams/knowledge-access/policy","GET",undefined,session));}
export async function writeKnowledgeAccessPolicy(client:Client,session:string,body:WriteKnowledgeAccessPolicy){return validatePolicy(await productRequest<unknown>(client,"/teams/knowledge-access/policy","PUT",body,session));}
export async function actorKnowledgeAccess(client:Client,session:string){return validateActorAccess(await productRequest<unknown>(client,"/teams/knowledge-access/me","GET",undefined,session));}
export async function teamProjectKnowledgeAccess(client:Client,session:string,team:string,project:string){
  const v=object(await productRequest<unknown>(client,`/teams/${encodeURIComponent(team)}/projects/${encodeURIComponent(project)}/knowledge-access`,"GET",undefined,session));
  keys(v,["project_id","project_knowledge"]);if(v.project_id!==project)throw new Error("knowledge_access_invalid");return {project_id:project,project_knowledge:bool(v.project_knowledge)};
}
/** Toggle one ID in a sorted, duplicate-free exception list. */
export function toggleException(list:string[],id:string,enabled:boolean){const next=new Set(list);if(enabled)next.add(id);else next.delete(id);return [...next].sort();}
export const knowledgeBaseDisabled=(error:unknown)=>error instanceof RequestError?[error.details.code,error.details.message].includes("knowledge_base_disabled"):String(error instanceof Error?error.message:error).includes("knowledge_base_disabled");
