import {useEffect,useId,useMemo,useRef,useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {Client,RequestError,type WorkflowInputView,type WorkflowInputDraft,type WorkflowInputDraftSave,type WorkflowInputAnswerIntent,type WorkflowInputDraftSnapshot,type WorkflowInputDraftSnapshotSummary,type WorkflowRunView} from "@cowork/client";
import {de,type Key} from "./i18n";
import {workflowClientIdentity} from "./workflowJournal";
import {WorkflowInputSession,workflowAnswerBytes,workflowAnswerValid} from "./workflowInputSession";
import {WorkflowReviewAnswerEditor,WorkflowReviewAnswerPreview} from "./WorkflowReviewForm";
import {WorkflowReviewSources} from "./WorkflowReviewSources";
import {reviewAnswerValid} from "./workflowReviews";
import {useView} from "./view";

type T=(key:Key)=>string;
type Props={client:Client;view:WorkflowRunView;t:T;busy:boolean;changed:()=>Promise<void>};
export function WorkflowInputsPanel({client,view,t,busy,changed}:Props){
  const inputs=view.inputs??[];if(!inputs.length)return null;
  return <section className="workflow-inputs"><h2>{t("workflowInputsTitle")}</h2>{inputs.map(input=>{
    const node=view.nodes.find(value=>value.id===input.node_id);
    return <WorkflowInputAnswer key={[workflowClientIdentity(client),view.run.spec.project_id,view.run.id,input.request_id,input.fingerprint].join(":")} client={client} project={view.run.spec.project_id} run={view.run.id} input={input} goal={node?.goal??""} t={t} busy={busy} changed={changed}/>;
  })}</section>;
}
type CardProps={client:Client;project:string;run:string;input:WorkflowInputView;goal:string;t:T;busy:boolean;changed:()=>Promise<void>};
type Conflict=Extract<WorkflowInputDraftSave,{outcome:"conflict"}>;
type SnapshotSelection={session:WorkflowInputSession;value:WorkflowInputDraftSnapshot;head:WorkflowInputDraft};
type Editor={session:WorkflowInputSession;answer:string;head:WorkflowInputDraft|null;baseline:string;conflict:Conflict|null};
export function WorkflowInputAnswer({client,project,run,input,goal,t,busy,changed}:CardProps){
  const id=useId();const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const session=useMemo(()=>new WorkflowInputSession(client,project,run,input.request_id,input.fingerprint,input.review?.values),[client,project,run,input.request_id,input.fingerprint]);
  const active=useRef<WorkflowInputSession|null>(session);active.current=session;
  useEffect(()=>{session.activate();active.current=session;setActivity({session,busy:false,error:null});return()=>{session.dispose();if(active.current===session)active.current=null;};},[session]);
  const lookup=useQuery({queryKey:["workflow-input-draft",workflowClientIdentity(client),project,run,input.request_id,input.fingerprint],queryFn:()=>session.read(input.review?input.receipt:undefined),retry:false,refetchOnMount:"always",refetchInterval:3000});
  const [editor,setEditor]=useState<Editor>({session,answer:"",head:null,baseline:"",conflict:null});
  const current=editor.session===session?editor:{session,answer:"",head:null,baseline:"",conflict:null};
  const editorRef=useRef(current);editorRef.current=current;
  const [intent,setIntent]=useState<{session:WorkflowInputSession;value:WorkflowInputAnswerIntent|null}>({session,value:null});
  const frozen=intent.session===session?intent.value:null;
  const [selection,setSelection]=useState<SnapshotSelection|null>(null);
  const selected=selection?.session===session?selection:null;
  const selectionSequence=useRef(0);
  useEffect(()=>{selectionSequence.current++;return()=>{selectionSequence.current++;};},[session]);
  const [activity,setActivity]=useState<{session:WorkflowInputSession;busy:boolean;error:unknown}>({session,busy:false,error:null});
  const pending=activity.session===session&&activity.busy;
  const error=activity.session===session?activity.error:null;
  const inflight=useRef<WorkflowInputSession|null>(null);
  useEffect(()=>{if(!lookup.data||active.current!==session)return;
    setEditor(previous=>previous.session===session&&previous.head?previous:{session,answer:lookup.data!.draft.answer,head:lookup.data!.draft,baseline:lookup.data!.draft.answer,conflict:null});
    setIntent({session,value:session.intent});
  },[lookup.data,session]);
  const dirty=!!current.head&&(current.answer!==current.baseline||!!input.review&&current.head.revision===0);
  const bytes=workflowAnswerBytes(current.answer);
  const draftValid=bytes<=32_000&&!current.answer.includes("\0");
  const remote=lookup.data?.draft;
  const sameRevisionInvalid=!!remote&&!!current.head&&remote.revision===current.head.revision&&(remote.snapshot_id!==current.head.snapshot_id||remote.answer_sha256!==current.head.answer_sha256||remote.answer!==current.head.answer);
  const remoteChanged=!!remote&&!!current.head&&remote.revision>current.head.revision;
  const confirmedLookup=lookup.isSuccess&&!lookup.isFetching&&!sameRevisionInvalid;
  const pendingIntentKnown=lookup.data?.intents.some(value=>value.state==="pending")??false;
  const editable=input.state==="waiting_answer"||input.state==="closed"&&input.closed_reason==="interrupted";
  const actionable=input.state==="waiting_answer"&&input.actionable;
  const apply=(value:Editor)=>{if(active.current===session){editorRef.current=value;setEditor(value);}};
  async function perform(task:()=>Promise<void>){
    if(active.current!==session||!session.isCurrent(session.generation)||busy||inflight.current===session)return;const generation=session.generation;inflight.current=session;setActivity({session,busy:true,error:null});
    try{await task();}catch(reason){if(active.current===session&&session.isCurrent(generation)){setActivity({session,busy:true,error:reason});setIntent({session,value:session.intent});}}
    finally{if(inflight.current===session)inflight.current=null;if(active.current===session&&session.isCurrent(generation))setActivity(previous=>previous.session===session?{...previous,busy:false}:previous);}
  }
  async function save(){
    const generation=session.generation;const value=editorRef.current;if(active.current!==session||value.session!==session||!value.head||(!editable&&!session.pendingSave)||(!draftValid&&!session.pendingSave)||!confirmedLookup)return;
    const result=await session.save(value.answer,value.head.revision);if(active.current!==session||!session.isCurrent(generation))return;
    const latest=editorRef.current;
    if(result.outcome==="conflict")apply({...latest,conflict:result});
    else apply({...latest,head:result.draft,baseline:result.snapshot.answer,conflict:null});
    void lookup.refetch();
  }
  useEffect(()=>{if(!dirty||!editable||!draftValid||pending||busy||current.conflict||remoteChanged||session.pendingSave||session.uncertainPrepare||!confirmedLookup)return;
    const timer=setTimeout(()=>void perform(save),500);return()=>clearTimeout(timer);
  },[dirty,current.answer,editable,draftValid,pending,busy,current.conflict,remoteChanged,session,confirmedLookup]);
  async function prepare(){
    const generation=session.generation;if(!actionable||!current.head||current.conflict||remoteChanged||!confirmedLookup||(!workflowAnswerValid(current.answer)||!reviewAnswerValid(input.review,current.answer)))return;
    if(dirty||session.pendingSave)await save();if(active.current!==session||!session.isCurrent(generation))return;
    const latest=editorRef.current;if(!latest.head||latest.conflict||latest.answer!==latest.baseline||session.pendingSave)return;
    const value=await session.prepare(latest.head.revision,latest.answer);if(active.current===session&&session.isCurrent(generation)){setIntent({session,value});void lookup.refetch();}
  }
  async function commit(){
    const generation=session.generation;if(!actionable||!frozen||frozen.state!=="pending"||!confirmedLookup)return;
    try{const value=await session.commit();if(active.current===session&&session.isCurrent(generation)){setIntent({session,value});await changed();}}
    finally{if(active.current===session&&session.isCurrent(generation))void lookup.refetch();}
  }
  async function choose(answer:string|null){
    if(!editable||session.pendingSave||session.uncertainPrepare)return;const generation=session.generation;
    const latest=await session.currentDraft();if(active.current!==session||!session.isCurrent(generation))return;
    apply({session,head:latest.draft,answer:answer??latest.draft.answer,baseline:latest.draft.answer,conflict:null});setSelection(null);void lookup.refetch();
  }
  async function inspectIntent(intentId:string){const generation=session.generation;const value=await session.inspect(intentId);if(active.current===session&&session.isCurrent(generation))setIntent({session,value});}
  async function inspectSnapshot(summary:WorkflowInputDraftSnapshotSummary){
    const generation=session.generation,sequence=++selectionSequence.current;
    const value=await session.snapshot(summary);const latest=await session.currentDraft();
    if(active.current===session&&session.isCurrent(generation)&&sequence===selectionSequence.current)setSelection({session,value,head:latest.draft});
  }
  async function retryPreparation(){if(!current.head||!session.uncertainPrepare)return;const generation=session.generation;const value=await session.prepare(current.head.revision,current.answer);if(active.current===session&&session.isCurrent(generation)){setIntent({session,value});void lookup.refetch();}}

  return <section className="workflow-input-card" aria-labelledby={`${id}-title`}><h3 id={`${id}-title`}>{input.role_id??tr("Prüfung","Review")}{goal?" · "+goal:""}</h3>
    <p className="workflow-input-question">{input.question}</p><p className="muted">{t("workflowInputDeadline")}: {new Date(input.expires_at).toLocaleString(language)}</p>
    <div className={`workflow-review-layout${input.review?.evidence?.length?" with-sources":""}`}>
    {input.review&&<WorkflowReviewSources client={client} run={run} review={input.review}/>}
    <div className="workflow-review-answer">
    {!actionable&&<p role="status">{t(input.state==="answered"?"workflowInputAccepted":editable?"workflowInputInterruptedDraft":"workflowInputClosed")}</p>}
    {!current.head&&<p role={lookup.isError?"alert":"status"}>{t(lookup.isError?(lookup.error instanceof Error&&lookup.error.message==="workflowInputMultiplePending"?"workflowInputMultiplePending":"workflowInputDraftUnavailable"):"workflowInputDraftLoading")}</p>}
    {current.head&&!(input.review&&input.state==="answered")&&<>{input.review?<WorkflowReviewAnswerEditor review={input.review} answer={current.answer} disabled={!editable||busy||!!current.conflict||remoteChanged} onChange={answer=>apply({...editorRef.current,answer})}/>:<><label htmlFor={`${id}-answer`}>{t("workflowInputAnswer")}</label><textarea id={`${id}-answer`} rows={4} value={current.answer} disabled={!editable||busy||!!current.conflict||remoteChanged} onChange={event=>apply({...editorRef.current,answer:event.target.value})}/></>}
      <p className="muted">{t("workflowInputBytes")}: {bytes.toLocaleString(language)} / 32.000</p>{!draftValid&&<p role="alert">{t("workflowInputLimit")}</p>}
      <p role="status">{t(pending?"saving":lookup.isError||sameRevisionInvalid?"workflowInputBinding":dirty||session.pendingSave?"workflowInputUnsaved":"workflowInputDraftSaved")}</p>
      {(editable||session.pendingSave)&&<div className="actions"><button type="button" disabled={busy||pending||(!draftValid&&!session.pendingSave)||!!current.conflict&&!session.pendingSave||remoteChanged&&!session.pendingSave||!confirmedLookup||(!dirty&&!session.pendingSave)} onClick={()=>void perform(save)}>{t(session.pendingSave?"workflowInputRetrySave":"workflowInputSave")}</button>
        <button type="button" disabled={busy||pending||!actionable||!!frozen||pendingIntentKnown||!!current.conflict||remoteChanged||(!workflowAnswerValid(current.answer)||!reviewAnswerValid(input.review,current.answer))||!confirmedLookup} onClick={()=>void perform(prepare)}>{t(session.uncertainPrepare?"workflowInputRetryIntent":"workflowInputPrepare")}</button></div>}
    </>}
    {(current.conflict||remoteChanged)&&<section className="conflict" role="alert"><p>{t("workflowInputConflict")}</p><h4>{t("workflowInputServerCopy")}</h4><pre>{(current.conflict?.draft??remote)?.answer}</pre><h4>{t("workflowInputIncomingCopy")}</h4><pre>{current.conflict?.snapshot.answer??current.answer}</pre>
      <div className="actions"><button type="button" disabled={busy||pending||!editable||session.pendingSave||session.uncertainPrepare} onClick={()=>void perform(()=>choose(null))}>{t("workflowInputUseServer")}</button>
      <button type="button" disabled={busy||pending||!editable||session.pendingSave||session.uncertainPrepare} onClick={()=>void perform(()=>choose(current.conflict?.snapshot.answer??current.answer))}>{t("workflowInputUseMine")}</button></div></section>}
    {session.unconfirmedSave&&<section role="alert"><p>{t("workflowInputUnconfirmedSave")}</p><pre>{session.unconfirmedSave.answer}</pre></section>}
    {session.uncertainPrepare&&<section role="alert"><p>{t("workflowInputPrepareUncertain")}</p><button type="button" disabled={busy||pending||!current.head} onClick={()=>void perform(retryPreparation)}>{t("workflowInputRetryIntent")}</button></section>}
    {frozen&&<section className="workflow-input-intent" aria-label={t("workflowInputFrozenAnswer")}><h4>{t("workflowInputFrozenAnswer")}</h4>{input.review?<WorkflowReviewAnswerPreview form={input.review.form} answer={frozen.submission.input.answer}/>:<pre>{frozen.submission.input.answer}</pre>}<p role="status">{t(frozen.state==="pending"?"workflowInputPrepared":frozen.state==="accepted"?"workflowInputAccepted":"workflowInputRejected")}</p>
      {frozen.rejection&&<p>{t(frozen.rejection.code in de?frozen.rejection.code as Key:"error")}</p>}
      <div className="actions"><button type="button" disabled={busy||pending} onClick={()=>void perform(()=>inspectIntent(frozen.id))}>{t("workflowInputRetryIntent")}</button>
        {frozen.state==="pending"&&<button type="button" className="primary" disabled={busy||pending||!actionable||!confirmedLookup} onClick={()=>void perform(commit)}>{t("workflowInputSend")}</button>}
        {frozen.state==="rejected"&&actionable&&<button type="button" disabled={busy||pending||!session.canContinueRejected} onClick={()=>{session.continueRejected();setIntent({session,value:null});void lookup.refetch();}}>{t("workflowInputContinue")}</button>}</div>
      <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({intent_id:frozen.id,request_sha256:frozen.request_sha256,draft_snapshot:frozen.draft_snapshot,receipt:frozen.receipt,rejection:frozen.rejection},null,2)}</pre></details></section>}
    {!!lookup.data?.intents.length&&<details><summary>{t("workflowInputHistory")}</summary>{lookup.data.intents.map(value=><button type="button" key={value.id} disabled={busy||pending} onClick={()=>void perform(()=>inspectIntent(value.id))}>{t(value.state==="pending"?"workflowInputPrepared":value.state==="accepted"?"workflowInputAccepted":"workflowInputRejected")} · {new Date(value.created_at).toLocaleString(language)}</button>)}</details>}
    {!!lookup.data?.snapshots.length&&<details><summary>{t("workflowInputSnapshots")}</summary>{lookup.data.snapshots.map(value=><button type="button" key={value.id} disabled={busy||pending} onClick={()=>void perform(()=>inspectSnapshot(value))}>{t(value.conflict?"workflowInputIncomingCopy":"workflowInputSavedCopy")} · {new Date(value.created_at).toLocaleString(language)}</button>)}</details>}
    {selected&&<WorkflowInputSnapshotView value={selected.value} head={selected.head} t={t} disabled={busy||pending||!editable||session.pendingSave||session.uncertainPrepare} useCopy={()=>void perform(()=>choose(selected.value.answer))}/>}
    {sameRevisionInvalid&&<p role="alert">{t("workflowInputBinding")}</p>}
    <button type="button" disabled={busy||pending||lookup.isFetching} onClick={()=>void lookup.refetch()}>{t("workflowInputRefresh")}</button>
    {lookup.isError&&current.head&&<p role="alert">{t(lookup.error instanceof Error&&lookup.error.message==="workflowInputMultiplePending"?"workflowInputMultiplePending":"workflowInputDraftUnavailable")}</p>}
    {!!error&&<p role="alert">{t(error instanceof RequestError&&error.details.code==="budget_exceeded"?"workflowInputQuota":error instanceof RequestError&&error.details.code in de?error.details.code as Key:error instanceof Error&&error.message in de?error.message as Key:"workflowInputDraftUnavailable")}</p>}
    <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({request_id:input.request_id,fingerprint:input.fingerprint,node_id:input.node_id,state:input.state,closed_reason:input.closed_reason,draft_revision:current.head?.revision,snapshot_id:current.head?.snapshot_id},null,2)}</pre></details>
    </div></div>
  </section>;
}

export function WorkflowInputSnapshotView({value,head,t,disabled,useCopy}:{value:WorkflowInputDraftSnapshot;head:WorkflowInputDraft;t:T;disabled:boolean;useCopy:()=>void}){
  return <section className="workflow-input-snapshot" aria-label={t("workflowInputSnapshotPreview")}><h4>{t(value.conflict?"workflowInputIncomingCopy":"workflowInputSavedCopy")}</h4><pre>{value.answer}</pre><h4>{t("workflowInputServerCopy")}</h4><pre>{head.answer}</pre><p>{t("workflowInputSnapshotHint")}</p><button type="button" disabled={disabled} onClick={useCopy}>{t("workflowInputRestoreSnapshot")}</button><details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({snapshot_id:value.id,base_revision:value.base_revision,revision:value.revision,answer_sha256:value.answer_sha256,current_revision:head.revision},null,2)}</pre></details></section>;
}
