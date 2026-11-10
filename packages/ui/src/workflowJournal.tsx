import {confirmedWorkflowStart} from "./composerLayout";
import {useCallback, useEffect, useMemo, useRef, useState} from "react";
import {useQuery, useQueryClient} from "@tanstack/react-query";
import {Client, RequestError, decode, type PrepareWorkflowOperation, type WorkflowDraft, type WorkflowDraftContent, type WorkflowDraftSnapshotRef, type WorkflowOperation, type WorkflowOperationRequest} from "@cowork/client";
import type {Key} from "./i18n";

export const workflowEditorFormat="cowork.workflow-editor.v1";
/** Equality of editor JSON ignores object key order and preserves array order and data values. */
export function workflowDraftKey(value:unknown):string {
  const canonical=(item:unknown):unknown=>Array.isArray(item)?item.map(canonical):item&&typeof item==="object"?Object.fromEntries(Object.keys(item).sort().map(key=>[key,canonical((item as Record<string,unknown>)[key])])):item;
  return JSON.stringify(canonical(value));
}
type T=(key:Key)=>string;
const clients=new WeakMap<Client,number>();let clientSequence=0;
export function workflowClientIdentity(client:Client){let identity=clients.get(client);if(identity===undefined){identity=++clientSequence;clients.set(client,identity);}return identity;}
async function pages<T>(get:(offset:number)=>Promise<{items:T[];next_offset?:number|null}>):Promise<T[]> {
  const items:T[]=[];let offset=0;
  while(true){const page=await get(offset);items.push(...page.items);if(page.next_offset==null)return items;
    if(page.next_offset<=offset)throw new Error("workflowJournalUnavailable");offset=page.next_offset;}
}
export async function loadWorkflowRecovery(client:Client,project:string) {
  const [drafts,operations]=await Promise.all([pages(offset=>client.workflowDrafts(project,offset)),pages(offset=>client.workflowOperations(project,offset))]);
  return {drafts,operations};
}
/** Editor data may contain temporary invalid numeric inputs; validation here checks shape, not permission. */
export function restoreWorkflowEditor(content:WorkflowDraftContent) {
  if(content.editor_format!==workflowEditorFormat)return null;
  const data=content.data;
  if(!data||typeof data!=="object"||Array.isArray(data))return null;
  const fields=data as Record<string,unknown>;
  if(![1,2,3,4,5].includes(fields.format_version as number)||!Array.isArray(fields.roles)||!Array.isArray(fields.steps)||fields.roles.length>32||fields.steps.length>64)return null;
  const normalize=(value:unknown):unknown=>{
    if(typeof value==="number"){if(!Number.isFinite(value))throw new Error("invalid_editor_number");return Math.max(0,Math.floor(value));}
    if(Array.isArray(value))return value.map(normalize);
    if(value&&typeof value==="object")return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,normalize(item)]));
    return value;
  };
  try {decode("WorkflowDefinition",normalize(data));return structuredClone(data) as ReturnType<typeof decode<"WorkflowDefinition">>;}catch{return null;}
}
export class WorkflowRecoveryRequired extends Error {
  constructor(readonly operation:WorkflowOperation){super("workflowJournalRecovered");}
}
function resumeOperationBinding(operation:WorkflowOperation,previous:WorkflowOperation|null){
  if(operation.request.kind!=="resume_input")return;
  const request=operation.request.request,receipt=operation.receipt;
  const identity=(value:WorkflowOperation)=>workflowDraftKey({id:value.id,project_id:value.project_id,intent_id:value.intent_id,
    request_sha256:value.request_sha256,request:value.request,draft_snapshot:value.draft_snapshot??null,created_at:value.created_at});
  if(previous&&identity(operation)!==identity(previous))throw new Error("contract_mismatch:workflow_operation");
  if(operation.state!=="accepted"){
    if(receipt!=null)throw new Error("contract_mismatch:workflow_operation");
    return;
  }
  if(receipt?.kind!=="input_resume_queued"||receipt.run_id!==request.run_id||receipt.resume_id!==operation.id
      ||receipt.cursor_sha256!==request.expected_cursor_sha256||receipt.accepted_run_revision!==request.base_run_revision+1
      ||receipt.accepted_workflow_revision!==request.base_workflow_revision+1||receipt.accepted_at!==operation.updated_at
      ||!Number.isSafeInteger(receipt.generation)||receipt.generation<1||!Number.isSafeInteger(receipt.confirmed_sequence)||receipt.confirmed_sequence<1
      ||previous?.state==="accepted"&&workflowDraftKey(previous.receipt)!==workflowDraftKey(receipt))throw new Error("contract_mismatch:workflow_operation");
}
/** The service owns durability. This object only holds an identical prepare until its acknowledgement arrives. */
export class WorkflowOperationSession {
  operation:WorkflowOperation|null=null;
  private prepared:PrepareWorkflowOperation|null=null;
  private preparing:Promise<WorkflowOperation>|null=null;
  constructor(private client:Client,private project:string,private matches:(operation:WorkflowOperation)=>boolean){}
  get uncertainPrepare(){return this.prepared!==null&&this.operation===null;}
  attach(operation:WorkflowOperation){resumeOperationBinding(operation,this.operation);this.operation=structuredClone(operation);if(this.prepared?.intent_id===operation.intent_id)this.prepared=null;}
  prepare(build:()=>WorkflowOperationRequest,snapshot?:WorkflowDraftSnapshotRef):Promise<WorkflowOperation> {
    if(this.preparing)return this.preparing;
    const task=this.prepareOnce(build,snapshot);this.preparing=task;
    void task.finally(()=>{if(this.preparing===task)this.preparing=null;}).catch(()=>{});
    return task;
  }
  private async prepareOnce(build:()=>WorkflowOperationRequest,snapshot?:WorkflowDraftSnapshotRef):Promise<WorkflowOperation> {
    if(this.operation)return this.operation;
    const operations=await pages(offset=>this.client.workflowOperations(this.project,offset));
    if(this.prepared){
      const confirmed=operations.find(item=>item.intent_id===this.prepared!.intent_id);
      if(confirmed){this.attach(confirmed);return confirmed;}
    }else{
      const existing=operations.find(item=>this.matches(item)&&item.acknowledged_at==null);
      if(existing){this.attach(existing);throw new WorkflowRecoveryRequired(existing);}
      this.prepared=structuredClone({intent_id:crypto.randomUUID(),request:build(),draft_snapshot:snapshot??null});
    }
    const result=await this.client.prepareWorkflowOperation(this.project,this.prepared);
    if(result.project_id!==this.project||result.intent_id!==this.prepared.intent_id||result.request.kind!==this.prepared.request.kind||result.request.request.idempotency_key!==this.prepared.request.request.idempotency_key||!this.matches(result))throw new Error("contract_mismatch:workflow_operation");
    this.attach(result);return result;
  }
  private async change(kind:"commitWorkflowOperation"|"discardWorkflowOperation"){
    if(!this.operation)throw new Error("workflowJournalUnavailable");const id=this.operation.id;
    try {const result=await this.client[kind](this.project,id);if(result.id!==id||result.project_id!==this.project)throw new Error("contract_mismatch:workflow_operation");this.attach(result);return result;}
    catch(reason){try {const current=await this.client.workflowOperation(this.project,id);if(current.id===id&&current.project_id===this.project)this.attach(current);}catch{/* Retain the original Pending record if reconciliation is unreachable. */}throw reason;}
  }
  commit(){return this.change("commitWorkflowOperation");}
  discard(){return this.change("discardWorkflowOperation");}
  async acknowledge(){if(!this.operation||this.operation.state==="pending")throw new Error("workflowJournalUnavailable");
    const id=this.operation.id;const state=this.operation.state;
    const result=await this.client.acknowledgeWorkflowOperation(this.project,id);
    if(result.id!==id||result.project_id!==this.project||result.state!==state)throw new Error("contract_mismatch:workflow_operation");
    if(result.acknowledged_at==null)throw new Error("workflowJournalUnavailable");
    this.operation=null;this.prepared=null;return result;}
}
export function useWorkflowRecovery(client:Client,project:string,matches:(operation:WorkflowOperation)=>boolean) {
  const qc=useQueryClient();
  const session=useMemo(()=>new WorkflowOperationSession(client,project,matches),[client,project,matches]);
  const key=useMemo(()=>["workflow-recovery",project,workflowClientIdentity(client)] as const,[client,project]);
  const active=useRef<WorkflowOperationSession|null>(session);active.current=session;
  useEffect(()=>{active.current=session;return()=>{if(active.current===session)active.current=null;};},[session]);
  const query=useQuery({queryKey:key,queryFn:()=>loadWorkflowRecovery(client,project),retry:false,refetchOnMount:"always"});
  const [selection,setSelection]=useState<{session:WorkflowOperationSession;operation:WorkflowOperation|null}>({session,operation:null});
  const operation=selection.session===session?selection.operation:null;
  const setOperation=(value:WorkflowOperation|null)=>{if(active.current===session)setSelection({session,operation:value});};
  const [activity,setActivity]=useState<{session:WorkflowOperationSession;busy:boolean;error:unknown}>({session,busy:false,error:undefined});
  const busy=activity.session===session&&activity.busy;
  const error=activity.session===session?activity.error:undefined;
  const inflight=useRef<WorkflowOperationSession|null>(null);
  const status=(busy:boolean,error:unknown=undefined)=>{if(active.current===session)setActivity({session,busy,error});};
  useEffect(()=>{if(operation||session.uncertainPrepare)return;const recovered=query.data?.operations.find(item=>matches(item)&&item.acknowledged_at==null);
    if(recovered){session.attach(recovered);setOperation(recovered);}},[query.data,operation,session,matches]);
  const update=(value:WorkflowOperation)=>{if(active.current!==session)return;setOperation(value);qc.setQueryData<Awaited<ReturnType<typeof loadWorkflowRecovery>>>(key,old=>old?{...old,operations:[value,...old.operations.filter(item=>item.id!==value.id)]}:old);};
  async function begin(build:()=>WorkflowOperationRequest,snapshot?:WorkflowDraftSnapshotRef) {
    if(inflight.current===session)return;inflight.current=session;status(true);
    try {const prepared=await session.prepare(build,snapshot);update(prepared);
      // Yield a render with the confirmed Pending record before the commit request.
      await new Promise<void>(resolve=>setTimeout(resolve,0));
      if(prepared.state==="pending"&&active.current===session)update(await session.commit());
    }catch(reason){if(reason instanceof WorkflowRecoveryRequired){update(reason.operation);status(false);}else{if(session.operation)update(session.operation);status(false,reason);}}finally{if(inflight.current===session)inflight.current=null;if(active.current===session)setActivity(old=>old.session===session?{...old,busy:false}:old);}
  }
  async function action(kind:"commit"|"discard"|"acknowledge",accept?:(operation:WorkflowOperation)=>Promise<void>) {
    if(inflight.current===session)return;inflight.current=session;status(true);
    try{
      if(kind==="acknowledge"){
        if(operation?.state==="accepted"&&accept)await accept(operation);
        const result=await session.acknowledge();
        qc.setQueryData<Awaited<ReturnType<typeof loadWorkflowRecovery>>>(key,old=>old?{...old,operations:old.operations.filter(item=>item.id!==result.id)}:old);
        setOperation(null);
      }else update(await session[kind]());
    }catch(reason){if(session.operation)update(session.operation);status(false,reason);}finally{if(inflight.current===session)inflight.current=null;if(active.current===session)setActivity(old=>old.session===session?{...old,busy:false}:old);}
  }
  const rememberDraft=useCallback((draft:WorkflowDraft)=>{qc.setQueryData<Awaited<ReturnType<typeof loadWorkflowRecovery>>>(key,old=>old?{...old,drafts:[draft,...old.drafts.filter(item=>item.id!==draft.id)]}:old);},[qc,key]);
  return {query,operation,busy,error,begin,action,rememberDraft,uncertainPrepare:session.uncertainPrepare,
    ready:query.isSuccess&&!query.isFetching&&!operation&&!session.uncertainPrepare&&!query.data.operations.some(item=>matches(item)&&item.acknowledged_at==null)};
}
export function WorkflowOperationPanel({operation,busy,error,t,commit,discard,acknowledge,children,compactAcceptedStart=false}:{operation:WorkflowOperation|null;busy:boolean;error:unknown;t:T;commit:()=>void;discard:()=>void;acknowledge:()=>void;children?:React.ReactNode;compactAcceptedStart?:boolean}) {
  const [confirm,setConfirm]=useState(false);
  useEffect(()=>setConfirm(false),[operation?.id,operation?.state]);
  if(!operation)return null;
  const name=operation.request.kind==="resume_input"?t("workflowInputResume"):operation.request.kind==="start"?operation.request.request.prompt:operation.request.kind==="revoke"?t("workflowRevoke"):operation.request.request.definition.name;
  if(compactAcceptedStart&&confirmedWorkflowStart(operation)&&!error)return <section className="banner workflow-recovery workflow-start-receipt" aria-label={t("workflowJournalTitle")}>
    <header><strong>{t("workflowStartAccepted")}</strong><button disabled={busy} onClick={acknowledge}>{t("workflowJournalOpen")}</button></header>
    <details><summary>{t("workflowStartReceipt")}</summary><p>{t("workflowJournalAccepted")}</p><p className="wrap">{name}</p>{children}
      <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(operation,null,2)}</pre></details></details></section>;
  return <section className="banner workflow-recovery" aria-label={t("workflowJournalTitle")}><h3>{t("workflowJournalTitle")}</h3>
    <p>{t(operation.state==="pending"?"workflowJournalPending":operation.state==="accepted"?"workflowJournalAccepted":operation.state==="rejected"?"workflowJournalRejected":"workflowJournalDiscarded")}</p>
    <p className="wrap">{name}</p>{children}
    {operation.rejection&&<p role="alert">{operation.rejection.message}</p>}
    {!!error&&<p role="alert">{t(error instanceof RequestError&&error.details.code==="budget_exceeded"?"workflowJournalQuota":"workflowJournalUnavailable")}</p>}
    <div className="actions">{operation.state==="pending"?<><button disabled={busy} onClick={commit}>{t("workflowRetrySame")}</button>
      {confirm?<><p>{t("workflowJournalDiscardHint")}</p><button disabled={busy} onClick={discard}>{t("workflowJournalDiscardConfirm")}</button><button disabled={busy} onClick={()=>setConfirm(false)}>{t("cancel")}</button></>:<button disabled={busy} onClick={()=>setConfirm(true)}>{t("workflowJournalDiscard")}</button>}</>:
      <button disabled={busy} onClick={acknowledge}>{t(operation.state==="accepted"?"workflowJournalOpen":"workflowJournalAcknowledge")}</button>}</div>
    <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(operation,null,2)}</pre></details></section>;
}
