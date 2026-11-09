import {decode,type Client,type DeleteLibraryEntry} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";

export type PendingChatDeletion={id:string;title:string;body:DeleteLibraryEntry};
export function validateChatDeletion(raw:unknown):PendingChatDeletion {
  if(!raw||typeof raw!=="object"||Array.isArray(raw)||Object.keys(raw).some(k=>!["id","title","body"].includes(k)))throw new Error("chat_deletion_reference_invalid");
  const value=raw as PendingChatDeletion,body=decode("DeleteLibraryEntry",value.body);
  if(typeof value.id!=="string"||!value.id||value.id.length>200||typeof value.title!=="string"||value.title.length>300||!Number.isSafeInteger(body.expected_revision)||body.expected_revision<1||!body.idempotency_key||body.idempotency_key.length>200)throw new Error("chat_deletion_reference_invalid");
  return {id:value.id,title:value.title,body};
}
export const chatDeletionLatch=(client:Client)=>new PersistentRequestLatch<PendingChatDeletion>(workspaceJournalKey(client,"delete-chat"),validateChatDeletion);
export async function submitChatDeletion(client:Client,latch:PersistentRequestLatch<PendingChatDeletion>,selected?:{id:string;title:string;revision:number}){
  const original=latch.prepare(()=>{if(!selected)throw new Error("chat_deletion_reference_missing");return {id:selected.id,title:selected.title,body:{expected_revision:selected.revision,idempotency_key:crypto.randomUUID()}};});
  const value=await client.one(`/orders/${encodeURIComponent(original.id)}/delete`,"LibraryDeleted","POST",original.body);
  if(!value.deleted||value.id!==original.id||value.revision!==original.body.expected_revision+1)throw new Error("chat_deletion_receipt_mismatch");
  latch.confirm();return value;
}
