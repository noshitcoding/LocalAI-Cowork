import {RequestError,type Client,type ProjectSettings,type WriteProjectSettings,type ChatManagement,type ManageChat,type ArchiveChat,type WorkspaceTask,type WriteWorkspaceTask,type StartWorkspaceTask,type ArchiveWorkspaceTask} from "@cowork/client";
import {PersistentRequestLatch,requestJournalEndpoint,definitiveRequestRejection,type RequestJournalStorage} from "./requestJournal";

export const projectSettings=async(client:Client,project:string)=>{const value=await client.one(`/projects/${encodeURIComponent(project)}/settings`,"ProjectSettings");if(value.project_id!==project)throw new Error("workspace_receipt_mismatch");return value;};
export const writeProjectSettings=async(client:Client,project:string,body:WriteProjectSettings)=>{const value=await client.one(`/projects/${encodeURIComponent(project)}/settings`,"ProjectSettings","PUT",body);if(value.project_id!==project||value.revision!==body.expected_revision+1)throw new Error("workspace_receipt_mismatch");return value;};
export const chatManagement=(client:Client,order:string)=>client.one(`/orders/${encodeURIComponent(order)}/management`,"ChatManagement");
export const archivedChats=(client:Client,project:string)=>client.many(`/projects/${encodeURIComponent(project)}/archived-chats`,"ChatManagement");
export const manageChat=async(client:Client,order:string,body:ManageChat)=>{const value=await client.one(`/orders/${encodeURIComponent(order)}/management`,"ChatManagement","PUT",body);if(value.order.id!==order||value.order.revision!==body.expected_revision+1||value.order.project_id!==body.project_id)throw new Error("workspace_receipt_mismatch");return value;};
export const archiveChat=async(client:Client,order:string,body:ArchiveChat)=>{const value=await client.one(`/orders/${encodeURIComponent(order)}/archive`,"ChatManagement","POST",body);if(value.order.id!==order||value.order.revision!==body.expected_revision+1||value.archived!==body.archived)throw new Error("workspace_receipt_mismatch");return value;};
export const workspaceTasks=(client:Client,project:string)=>client.many(`/projects/${encodeURIComponent(project)}/tasks`,"WorkspaceTask");
export const writeWorkspaceTask=async(client:Client,project:string,id:string|undefined,body:WriteWorkspaceTask)=>{const value=await client.one(id?`/workspace-tasks/${encodeURIComponent(id)}`:`/projects/${encodeURIComponent(project)}/tasks`,"WorkspaceTask",id?"PUT":"POST",body);if(id&&value.id!==id||value.project_id!==project||value.revision!==body.expected_revision+1)throw new Error("workspace_receipt_mismatch");return value;};
export const archiveWorkspaceTask=(client:Client,id:string,body:ArchiveWorkspaceTask)=>client.one(`/workspace-tasks/${encodeURIComponent(id)}`,"WorkspaceTask","DELETE",body);
export const startWorkspaceTask=(client:Client,id:string,body:StartWorkspaceTask)=>client.one(`/workspace-tasks/${encodeURIComponent(id)}/run`,"RunRecord","POST",body);
export const workspaceManagementKey=(client:Client)=>[client.base,client.token] as const;
export const confirmedWorkspaceRejection=(error:unknown)=>error instanceof RequestError&&definitiveRequestRejection(error.status);
/** Owns an exact request until the daemon returns a matching receipt. */
export class WorkspaceRequestLatch<T> extends PersistentRequestLatch<T>{constructor(key?:string,validate?:(value:unknown)=>T,storage?:RequestJournalStorage){super(key,validate,storage);}}
export const workspaceJournalKey=(client:Client,scope:string)=>"cowork.workspace.pending.v1:"+JSON.stringify([requestJournalEndpoint(client.base),"installation-owner",scope]);
export function projectInstructionSnapshot(settings:ProjectSettings):string {
  const sources=settings.sources.filter(source=>source.enabled).map(source=>`${source.label} (${source.kind}): ${source.reference} [${source.access}]`).join("\n");
  return [settings.instructions.trim(),sources&&`Project source references (metadata grants no file access):\n${sources}`].filter(Boolean).join("\n\n");
}
export type {ProjectSettings,WriteProjectSettings,ChatManagement,ManageChat,ArchiveChat,WorkspaceTask,WriteWorkspaceTask,StartWorkspaceTask,ArchiveWorkspaceTask};
