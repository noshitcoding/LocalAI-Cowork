import {useEffect,useRef,useState,type FormEvent} from "react";
import {useQueryClient} from "@tanstack/react-query";
import type {Client,Order,Project,ChatManagement,ManageChat,ArchiveChat} from "@cowork/client";
import {useView} from "./view";
import {DeleteChatButton} from "./ChatDeletion";
import {manageChat,archiveChat,confirmedWorkspaceRejection,WorkspaceRequestLatch} from "./workspaceManagementApi";
import {independentChats,visibleProjects} from "./chatScope";

export function ChatManagementDialog({client,order,projects,onChanged,onClose,blockClose}:{client:Client;order:Order;projects:Project[];onChanged:(value:ChatManagement)=>void;onClose:()=>void;blockClose?:(value:boolean)=>void}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b,qc=useQueryClient();
  const [title,setTitle]=useState(order.title),[project,setProject]=useState(order.project_id),[busy,setBusy]=useState(false),[pending,setPending]=useState(false),[confirm,setConfirm]=useState(false),[error,setError]=useState("");
  const latch=useRef(new WorkspaceRequestLatch<{archive:boolean;body:ManageChat|ArchiveChat}>());
  const initial=useRef(order).current,alive=useRef(true);useEffect(()=>{alive.current=true;return()=>{alive.current=false;};},[]);
  useEffect(()=>{blockClose?.(busy||pending);return()=>blockClose?.(false);},[busy,pending,blockClose]);
  async function submit(archive=false,event?:FormEvent){event?.preventDefault();if(busy)return;setBusy(true);setError("");try{
    const original=latch.current.prepare(()=>archive?{archive:true,body:{expected_revision:initial.revision,idempotency_key:crypto.randomUUID(),archived:true}}:{archive:false,body:{expected_revision:initial.revision,idempotency_key:crypto.randomUUID(),title,project_id:project}});setPending(true);
    if(!original.archive&&(original.body as ManageChat).project_id===independentChats)await client.one("/independent-chat-space","Project");
    const saved=original.archive?await archiveChat(client,initial.id,original.body as ArchiveChat):await manageChat(client,initial.id,original.body as ManageChat);if(!alive.current)return;latch.current.confirm();setPending(false);await Promise.all([qc.invalidateQueries({queryKey:["orders"]}),qc.invalidateQueries({queryKey:["order",initial.id]})]);if(!alive.current)return;onChanged(saved);onClose();
  }catch(reason){if(confirmedWorkspaceRejection(reason)){latch.current.confirm();setPending(false);}setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}}
  return <section className="product-stack chat-management">
    {error&&<p role="alert">{error}</p>}
    <form className="product-stack" onSubmit={event=>void submit(false,event)}>
      <fieldset disabled={busy||pending}>
        <label>{tr("Chatname","Chat title")}<input autoFocus required maxLength={300} value={title} onChange={e=>setTitle(e.target.value)}/></label>
        <label>{tr("Projekt","Project")}<select value={project} onChange={e=>setProject(e.target.value)}><option value={independentChats}>{tr("Ohne Projekt","Without project")}</option>{visibleProjects(projects).map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
      </fieldset>
      <button className="primary" disabled={busy}>{pending?tr("Dieselbe Anfrage abgleichen","Reconcile same request"):tr("Speichern","Save")}</button>
    </form>
    <p className="muted">{tr("Während eines aktiven Laufs kann der Chat nicht verschoben oder archiviert werden.","A chat cannot be moved or archived while it has an active run.")}</p>
    {confirm&&<div className="product-stack chat-archive-confirmation">
      <p>{tr("Chat archivieren? Nachrichten, Ergebnisse und Verlauf bleiben erhalten.","Archive this chat? Messages, results and history are preserved.")}</p>
      <button disabled={busy||pending} onClick={()=>void submit(true)}>{tr("Archivieren bestätigen","Confirm archive")}</button>
    </div>}
    <div className="actions">
      {!confirm&&<button disabled={busy||pending} onClick={()=>setConfirm(true)}>{tr("Archivieren","Archive")}</button>}
      <DeleteChatButton order={order} disabled={busy||pending} onSelected={onClose}/>
      <button disabled={busy||pending} onClick={onClose}>{tr("Schließen","Close")}</button>
    </div>
  </section>;
}
