import {useState} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import type {Client,ArchiveChat,ChatManagement} from "@cowork/client";
import {archiveChat,archivedChats,workspaceManagementKey,WorkspaceRequestLatch,workspaceJournalKey,confirmedWorkspaceRejection} from "./workspaceManagementApi";
import {decode} from "@cowork/client";
import {useView} from "./view";
import {DeleteChatButton} from "./ChatDeletion";
import {independentChats} from "./chatScope";
type Pending={id:string;body:ArchiveChat};
function restore(raw:unknown):Pending{if(!raw||typeof raw!=="object"||Array.isArray(raw)||Object.keys(raw).some(k=>!["id","body"].includes(k))||typeof (raw as Pending).id!=="string")throw new Error("request_journal_invalid");const value=raw as Pending;return {id:value.id,body:decode("ArchiveChat",value.body)};}
export function ArchivedChatsPanel({client,project,onRestored}:{client:Client;project:string;onRestored?:(value:ChatManagement)=>void}){
 const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b,qc=useQueryClient();
 const [latch]=useState(()=>new WorkspaceRequestLatch<Pending>(workspaceJournalKey(client,project+":restore-chat"),restore));
 const [pending,setPending]=useState(latch.pending),[busy,setBusy]=useState(false),[error,setError]=useState("");
 const query=useQuery({queryKey:["archived-chats",...workspaceManagementKey(client),project],queryFn:()=>archivedChats(client,project),enabled:!!project,retry:false});
 async function submit(item?:ChatManagement){if(busy)return;setBusy(true);setError("");try{const request=latch.prepare(()=>{if(!item)throw new Error("request_journal_invalid");return {id:item.order.id,body:{expected_revision:item.order.revision,idempotency_key:crypto.randomUUID(),archived:false}};});setPending(true);const value=await archiveChat(client,request.id,request.body);latch.confirm();setPending(false);await Promise.all([query.refetch(),qc.invalidateQueries({queryKey:["orders"]})]);onRestored?.(value);}catch(reason){if(confirmedWorkspaceRejection(reason)){latch.confirm();setPending(false);}setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}}
 return <section className="product-item"><h3>{tr("Archivierte Chats","Archived chats")}</h3>{(error||query.error)&&<p role="alert">{error||String(query.error)}</p>}{pending&&<p role="status">{tr("Der ursprüngliche Wiederherstellungsvorgang ist noch offen.","The original restore request is still pending.")} <button disabled={busy||latch.invalid} onClick={()=>void submit()}>{tr("Dieselbe Anfrage abgleichen","Reconcile same request")}</button>{latch.invalid&&<button disabled={busy} onClick={()=>{if(window.confirm(tr("Nur den lokalen Vorgangsverweis entfernen?","Remove only the local operation reference?"))){latch.discard();setPending(false);}}}>{tr("Lokalen Verweis entfernen","Remove local reference")}</button>}</p>}{query.data?.length===0&&<p>{project===independentChats?tr("Keine archivierten Chats.","No archived chats."):tr("Keine archivierten Chats in diesem Projekt.","No archived chats in this project.")}</p>}{query.data?.map(item=><article key={item.order.id}><strong>{item.order.title}</strong> <button disabled={busy||pending} onClick={()=>void submit(item)}>{tr("Wiederherstellen","Restore")}</button> <DeleteChatButton order={item.order} disabled={busy||pending}/></article>)}<button disabled={busy} onClick={()=>void query.refetch()}>{tr("Aktualisieren","Refresh")}</button></section>;
}
