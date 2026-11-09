import {WorkflowWorkspaceRight,WorkflowWorkspaceNotice} from "./WorkflowWorkspaceRight";
import {WorkflowDocumentsEditor} from "./WorkflowDocumentsEditor";
import {nativeReviewWorkflow} from "./workflowReviews";
import {WorkflowExtractionStarter} from "./WorkflowExtractionStarter";
import {nativeDocumentWorkflow} from "./workflowDocuments";
import {WorkflowIterationsEditor} from "./WorkflowIterationsEditor";
import {WorkflowSubflowsEditor} from "./WorkflowSubflowsEditor";
import {removeWorkflowNode} from "./workflowControls";
import {WorkflowControlsEditor} from "./WorkflowControlsEditor";
import {workflowDataNodes} from "./workflowDataflow";
import {useCallback, useEffect, useId, useRef, useState, type FormEvent} from "react";
import {useQuery, useQueryClient} from "@tanstack/react-query";
import {Client, RequestError, decode, defaultRunBudgets, type Budgets, type ProviderProfile, type WorkflowDefinition, type WorkflowLibraryEntry, type SaveWorkflowDraft, type WorkflowDraft, type WorkflowDraftOrigin, type WorkflowDraftSave, type WorkflowOperation, type UiLanguage} from "@cowork/client";
import {Plus, ArrowUp, ArrowDown, Layers} from "lucide-react";
import {RunBudgets} from "./RunBudgets";
import {WorkflowSnapshotRecovery} from "./WorkflowSnapshotPanel";
import {prepareIndependentSnapshotCopy,type WorkflowSnapshotRead} from "./workflowSnapshotRecovery";
import {de,type Key} from "./i18n";
import {deliverDownload,deliveryStatusKey} from "./download";
import {useView} from "./view";
import {StructuredOutputEditor} from "./StructuredOutputEditor";
import {ModelRegistryPicker} from "./ModelRegistryPicker";
import {productRequest} from "./productApi";
import {workflowTools, completeWorkflowBudgets, editableWorkflow, validWorkflowEditor, workflowRoleInstructionsValid, workflowRolePattern,renameWorkflowRole} from "./workflowUi";
import {useWorkflowRecovery, workflowEditorFormat, restoreWorkflowEditor, WorkflowOperationPanel,workflowClientIdentity,workflowDraftKey} from "./workflowJournal";
import {AgentLibrary,AgentLibrarySelector,useAgentLibrary} from "./AgentLibrary";
import {applyAgentToWorkflow,applyPersonalityToWorkflow,refreshWorkflowRoleFromLibrary,type Personality,type AgentTemplate} from "./libraryApi";
import {WorkflowStarterImport} from "./WorkflowStarterImport";
import {WorkflowDataflowEditor,WorkflowDataflowSummary} from "./WorkflowDataflowEditor";
import {reconcileWorkflowDataflow} from "./workflowDataflow";
import {bindCodexLibraryModel} from "./codexModelApi";
type T = (key: Key) => string;
type EditorDraft={id:string;revision:number;origin:WorkflowDraftOrigin|null;definition:WorkflowDefinition;saved:string;snapshot:WorkflowDraft["snapshot_id"]|null;sha256:string;archived:boolean};
const libraryOperations=(operation:WorkflowOperation)=>operation.request.kind==="create"||operation.request.kind==="publish"||operation.request.kind==="revoke";
const toolKeys: Partial<Record<typeof workflowTools[number], Key>> = {create_artifact:"workflowCreateArtifact",list_artifacts:"workflowListArtifacts",read_artifact:"workflowReadArtifact",request_input:"workflowRequestInput",document_read:"document_read",document_write:"document_write",image_edit:"image_edit"};
function blankDefinition(t:T):WorkflowDefinition {
  return {format_version:1,name:"",mode:{mode:"sequential"},failure:"continue_independent",rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},
    roles:[{id:"Agent",instructions:t("workflowDefaultInstructions"),provider_id:"",provider_revision:0,rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],
    steps:[{id:"step-"+crypto.randomUUID().slice(0,8),role_id:"Agent",goal:"",dependencies:[],rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets}}]};
}
const draftContent=(edit:EditorDraft)=>({editor_format:workflowEditorFormat,data:edit.definition,origin:edit.origin,archived:edit.archived});
export function exportWorkflowDraft(draft:unknown,language:UiLanguage){return deliverDownload(new Blob([JSON.stringify(draft,null,2)+"\n"],{type:"application/json;charset=utf-8"}),"workflow-draft.json",language);}
type WorkflowLibraryProps={client:Client;project:string;providers:ProviderProfile[];t:T;blockClose:(blocked:boolean)=>void;initialTab?:"workflows"|"personalities"|"agents"};
export function WorkflowLibrary(props:WorkflowLibraryProps) {
  return <WorkflowLibraryTabs key={props.project+":"+workflowClientIdentity(props.client)} {...props}/>;
}
function WorkflowLibraryTabs(props:WorkflowLibraryProps){
  const language=useView(v=>v.language),[tab,setTab]=useState<"workflows"|"personalities"|"agents">(props.initialTab??"workflows");
  const [blocked,setBlocked]=useState({workflows:false,personalities:false,agents:false});
  const workflowsBlocked=useCallback((value:boolean)=>setBlocked(current=>current.workflows===value?current:{...current,workflows:value}),[]);
  const personalitiesBlocked=useCallback((value:boolean)=>setBlocked(current=>current.personalities===value?current:{...current,personalities:value}),[]);
  const agentsBlocked=useCallback((value:boolean)=>setBlocked(current=>current.agents===value?current:{...current,agents:value}),[]);
  const anyBlocked=Object.values(blocked).some(Boolean);
  useEffect(()=>{props.blockClose(anyBlocked);return()=>props.blockClose(false);},[anyBlocked,props.blockClose]);
  const tabs=[{id:"workflows",label:"Workflows"},{id:"personalities",label:language==="de"?"Persönlichkeiten":"Personalities"},{id:"agents",label:language==="de"?"Agentvorlagen":"Agent templates"}] as const;
  return <section><nav className="workflow-toolbar" role="tablist" aria-label={language==="de"?"Bibliothek":"Library"}>{tabs.map(item=><button key={item.id} type="button" role="tab" aria-selected={tab===item.id} disabled={anyBlocked&&tab!==item.id} onClick={()=>setTab(item.id)}>{item.label}</button>)}</nav><div role="tabpanel" hidden={tab!=="workflows"}>{props.project?<WorkflowLibraryContext {...props} blockClose={workflowsBlocked}/>:<p>{language==="de"?"Wähle ein Projekt für Workflows. Persönlichkeiten und Agentvorlagen gelten projektübergreifend.":"Choose a project for workflows. Personalities and agent templates are available across projects."}</p>}</div>{tab==="personalities"&&<div role="tabpanel"><AgentLibrary {...props} kind="personalities" blockClose={personalitiesBlocked}/></div>}{tab==="agents"&&<div role="tabpanel"><AgentLibrary {...props} kind="agent-templates" blockClose={agentsBlocked}/></div>}</section>;
}
function WorkflowLibraryContext({client,project,providers,t,blockClose}:WorkflowLibraryProps) {
  const language=useView(value=>value.language);
  const tr=(de:string,en:string)=>language==="de"?de:en;
  const [exporting,setExporting]=useState(false);const [exportStatus,setExportStatus]=useState<Key|null>(null);const [exportError,setExportError]=useState<unknown>();
  async function exportDraft(draft:unknown){
    if(exporting)return;setExporting(true);setExportStatus(null);setExportError(undefined);
    try {setExportStatus(deliveryStatusKey(await exportWorkflowDraft(draft,language)));}
    catch(reason){setExportError(reason);}finally{setExporting(false);}
  }
  const qc=useQueryClient();const recovery=useWorkflowRecovery(client,project,libraryOperations);
  const [offset,setOffset]=useState(0);const [selected,setSelected]=useState("");const [versionId,setVersionId]=useState("");
  const [edit,setEdit]=useState<EditorDraft|null>(null);const editRef=useRef(edit);editRef.current=edit;
  const [unknownDraft,setUnknownDraft]=useState<WorkflowDraft|null>(null);
  type RestoredCopy={draft_id:string;operation_id:string;snapshot_id:string};
  const [restoredCopy,setRestoredCopyState]=useState<RestoredCopy|null>(null);const restoredCopyRef=useRef(restoredCopy);
  function setRestoredCopy(value:RestoredCopy|null){restoredCopyRef.current=value;setRestoredCopyState(value);}
  const [autosavePaused,setAutosavePaused]=useState(false);const [returnConfirm,setReturnConfirm]=useState(false);
  const [revokeConfirm,setRevokeConfirm]=useState(false);const [saving,setSaving]=useState(false);const [error,setError]=useState<unknown>();
  const [draftConflict,setDraftConflict]=useState<Extract<WorkflowDraftSave,{outcome:"conflict"}>|null>(null);
  const [conflicting,setConflicting]=useState<WorkflowLibraryEntry|null>(null);
  const saveRequest=useRef<SaveWorkflowDraft|null>(null);const saveTask=useRef<Promise<WorkflowDraftSave>|null>(null);
  const page=useQuery({queryKey:["workflows",project,offset],queryFn:()=>client.workflows(project,offset),retry:false});
  const entry=useQuery({queryKey:["workflow",project,selected],queryFn:()=>client.workflow(project,selected),enabled:!!selected,retry:false});
  const version=useQuery({queryKey:["workflow-version",project,selected,versionId],queryFn:()=>client.workflowVersion(project,selected,versionId),enabled:!!selected&&!!versionId,retry:false});
  const currentConflictVersion=useQuery({queryKey:["workflow-version",project,conflicting?.id,conflicting?.versions.at(-1)?.id],queryFn:async()=>{const head=conflicting!;const ref=head.versions.at(-1)!;const value=await client.workflowVersion(project,head.id,ref.id);if(value.project_id!==project||value.workflow_id!==head.id||value.reference.id!==ref.id||value.reference.sha256!==ref.sha256)throw new Error("workflowUnavailable");return value;},enabled:!!conflicting?.versions.length,retry:false});
  const [snapshotOffset,setSnapshotOffset]=useState(0);
  const snapshots=useQuery({queryKey:["workflow-draft-snapshots",project,edit?.id,snapshotOffset],queryFn:()=>client.workflowDraftSnapshots(project,edit!.id,snapshotOffset),enabled:!!edit&&edit.revision>0,retry:false});
  const dirty=!!edit&&workflowDraftKey(draftContent(edit))!==edit.saved;
  const blocked=saving||dirty||!!saveRequest.current;
  const busy=recovery.busy||saving||exporting;const frozen=!!recovery.operation||recovery.uncertainPrepare||!!recovery.query.data?.operations.some(operation=>libraryOperations(operation)&&operation.acknowledged_at==null);
  const ownCopy=!!edit&&restoredCopy?.draft_id===edit.id;const editorFrozen=frozen&&!ownCopy;
  useEffect(()=>{blockClose(blocked);return()=>blockClose(false);},[blocked,blockClose]);
  useEffect(()=>{if(!blocked)return;const warn=(event:BeforeUnloadEvent)=>event.preventDefault();window.addEventListener("beforeunload",warn);return()=>window.removeEventListener("beforeunload",warn);},[blocked]);
  const save=useCallback(async():Promise<WorkflowDraftSave>=>{
    if(saveTask.current)return saveTask.current;
    const current=editRef.current;if(!current)throw new Error("workflowDraftUnavailable");
    const input=saveRequest.current??{id:current.id,base_revision:current.revision,idempotency_key:crypto.randomUUID(),content:structuredClone(draftContent(current))};
    saveRequest.current=input;setSaving(true);setError(undefined);
    const task=client.saveWorkflowDraft(project,input);saveTask.current=task;
    try{
      const result=await task;saveRequest.current=null;recovery.rememberDraft(result.draft);
      if(result.outcome==="conflict"){setDraftConflict(result);setError(new Error("workflowDraftConflict"));}
      else {const latest=editRef.current;if(latest&&latest.id===input.id){const confirmed={...latest,revision:result.draft.revision,saved:workflowDraftKey(result.snapshot.content),snapshot:result.snapshot.id,sha256:result.snapshot.sha256};editRef.current=confirmed;setEdit(confirmed);}}
      void qc.invalidateQueries({queryKey:["workflow-draft-snapshots",project,input.id]});
      return result;
    }catch(reason){setError(reason);throw reason;}finally{if(saveTask.current===task)saveTask.current=null;setSaving(false);}
  },[client,project,qc,recovery.rememberDraft]);
  useEffect(()=>{if(!dirty||saving||draftConflict||editorFrozen||saveRequest.current||autosavePaused)return;const timer=setTimeout(()=>void save().catch(()=>{}),500);return()=>clearTimeout(timer);},[dirty,edit,saving,draftConflict,editorFrozen,save,autosavePaused]);
  function openDraft(draft:WorkflowDraft){
    setRestoredCopy(null);const definition=restoreWorkflowEditor(draft.content);setAutosavePaused(false);setReturnConfirm(false);setError(undefined);setDraftConflict(null);setConflicting(null);saveRequest.current=null;setSnapshotOffset(0);
    if(!definition||!editableWorkflow(definition)){setUnknownDraft(draft);setEdit(null);return;}
    setUnknownDraft(null);setEdit({id:draft.id,revision:draft.revision,origin:draft.content.origin??null,definition,saved:workflowDraftKey(draft.content),snapshot:draft.snapshot_id,sha256:draft.sha256,archived:draft.content.archived});
  }
  function newEditor(definition:WorkflowDefinition,origin:WorkflowDraftOrigin|null){
    if(!recovery.ready||blocked)return;setRestoredCopy(null);setAutosavePaused(false);setReturnConfirm(false);setEdit({id:crypto.randomUUID(),revision:0,origin,definition,saved:"",snapshot:null,sha256:"",archived:false});
    setUnknownDraft(null);setError(undefined);setConflicting(null);setDraftConflict(null);setRevokeConfirm(false);setSnapshotOffset(0);
  }
  async function returnToConfirmed(){
    if(!editRef.current||busy)return;setSaving(true);
    try {const current=await client.workflowDraft(project,editRef.current.id);recovery.rememberDraft(current);openDraft(current);}
    catch(reason){setError(reason);}finally{setSaving(false);}
  }
  async function write(kind:"save"|"revoke",event?:FormEvent){
    event?.preventDefault();if(busy)return;setError(undefined);
    try{
      if(frozen){await recovery.begin(()=>{throw new Error("workflowJournalUnavailable");});return;}
      if(kind==="revoke"){
        if(!entry.data)throw new Error("workflowUnavailable");
        const head=entry.data;
        await recovery.begin(()=>({kind:"revoke",workflow_id:head.id,request:{base_revision:head.revision,idempotency_key:crypto.randomUUID()}}));return;
      }
      if(!editRef.current||draftConflict||!validWorkflowEditor(editRef.current.definition)||!editableWorkflow(editRef.current.definition))throw new Error("workflowInvalid");
      // Snapshot exactly the form being published, including concurrent edits during saves.
      let snapshot;
      const current=editRef.current;
      if(current.snapshot&&workflowDraftKey(draftContent(current))===current.saved)snapshot=await client.workflowDraftSnapshot(project,current.id,current.snapshot);
      else {const result=await save();if(result.outcome==="conflict")return;snapshot=result.snapshot;}
      while(editRef.current&&workflowDraftKey(draftContent(editRef.current))!==workflowDraftKey(snapshot.content)){
        const result=await save();if(result.outcome==="conflict")return;snapshot=result.snapshot;
      }
      let definition:WorkflowDefinition;try{definition=decode("WorkflowDefinition",snapshot.content.data);}catch{throw new Error("workflowInvalid");}const origin=snapshot.content.origin;
      await recovery.begin(()=>origin?{kind:"publish",workflow_id:origin.workflow_id,request:{base_revision:origin.base_revision,idempotency_key:crypto.randomUUID(),definition}}:
        {kind:"create",request:{idempotency_key:crypto.randomUUID(),definition}},
        {draft_id:snapshot.draft_id,snapshot_id:snapshot.id,sha256:snapshot.sha256});
    }catch(reason){setError(reason);}
  }
  async function copySnapshot(value:WorkflowSnapshotRead){
    if(busy||blocked||recovery.uncertainPrepare||!recovery.operation||recovery.operation.id!==value.operation.id||value.operation.state==="pending")throw new Error("workflowSnapshotReadOnly");
    if(restoredCopyRef.current?.operation_id===value.operation.id&&restoredCopyRef.current.snapshot_id===value.snapshot?.id)return;
    const input=prepareIndependentSnapshotCopy(value);const content=input.content;const definition=restoreWorkflowEditor(content);
    if(!definition||!value.snapshot)throw new Error("workflowSnapshotReadOnly");
    const current:EditorDraft={id:input.id,revision:0,origin:content.origin??null,definition,saved:"",snapshot:null,sha256:"",archived:false};
    setRestoredCopy({draft_id:current.id,operation_id:value.operation.id,snapshot_id:value.snapshot.id});
    editRef.current=current;setEdit(current);setUnknownDraft(null);setDraftConflict(null);setConflicting(null);setError(undefined);setAutosavePaused(false);setSnapshotOffset(0);
    saveRequest.current=input;
    await save();
  }
  async function choosePublicationBase(){
    const current=editRef.current;if(!current?.origin||busy||blocked||!recovery.ready)return;
    setSaving(true);setError(undefined);
    try{
      const head=await client.workflow(project,current.origin.workflow_id);
      if(head.project_id!==project||head.id!==current.origin.workflow_id||!head.versions.length||head.revoked)throw new Error("workflowUnavailable");
      // Display a choice; reading a head never rewrites the origin or model selection.
      if(editRef.current?.id===current.id)setConflicting(head);
    }catch(reason){setError(reason);}finally{setSaving(false);}
  }
  async function accept(operation:WorkflowOperation){
    if(!operation.receipt||!("publication" in operation.receipt))throw new Error("workflowJournalUnavailable");
    const publication=operation.receipt.publication;setSelected(publication.workflow_id);if(publication.version_id)setVersionId(publication.version_id);
    if(restoredCopyRef.current?.draft_id!==editRef.current?.id||restoredCopyRef.current?.operation_id!==operation.id){setRestoredCopy(null);setEdit(null);setUnknownDraft(null);}
    setConflicting(null);setRevokeConfirm(false);
    await Promise.all([qc.invalidateQueries({queryKey:["workflows",project]}),qc.invalidateQueries({queryKey:["workflow-choices",project]}),qc.invalidateQueries({queryKey:["task-workflow-choices"]}),qc.invalidateQueries({queryKey:["schedule-workflow-choices"]}),qc.invalidateQueries({queryKey:["workflow",project,publication.workflow_id]})]);
  }
  async function acknowledge(){
    if(recovery.operation?.state==="rejected"&&edit?.origin){setConflicting(await client.workflow(project,edit.origin.workflow_id).catch(()=>null));}
    await recovery.action("acknowledge",accept);
  }
  function choose(value:WorkflowLibraryEntry){if(busy||blocked||frozen)return;setSelected(value.id);setVersionId(value.versions.at(-1)?.id??"");setEdit(null);setUnknownDraft(null);setError(undefined);setConflicting(null);setRevokeConfirm(false);}
  return <section className="workflow-library"><p>{t("workflowLibraryHint")}</p><p className="muted">{t("workflowLocalTools")}</p>
    {exportStatus&&<p role="status">{t(exportStatus)}</p>}{!!exportError&&<p role="alert">{t(exportError instanceof RequestError&&exportError.details.code in de?exportError.details.code as Key:"downloadFailed")}</p>}
    {!recovery.query.isSuccess&&<p role={recovery.query.isError?"alert":"status"}>{t(recovery.query.isError?"workflowJournalUnavailable":"workflowJournalLoading")}</p>}
    <div className="workflow-toolbar"><button type="button" disabled={!recovery.ready||busy||blocked||!!edit} onClick={()=>newEditor(blankDefinition(t),null)}><Plus size={16}/>{t("workflowNew")}</button><button type="button" disabled={!recovery.ready||busy||blocked||!!edit} onClick={()=>newEditor(nativeDocumentWorkflow(tr("Dokumentablauf ohne Modell","Document workflow without a model"),tr("Bericht erstellen","Create report"),tr("# Mein Bericht\n\nHier steht der Inhalt des Dokuments.","# My report\n\nWrite the document content here."),completeWorkflowBudgets(defaultRunBudgets)),null)}>{tr("Dokumentablauf ohne Modell","Document workflow without a model")}</button><button type="button" disabled={!recovery.ready||busy||blocked||!!edit} onClick={()=>newEditor(nativeReviewWorkflow(tr("Prüfung ohne Modell","Review without a model"),language,completeWorkflowBudgets(defaultRunBudgets)),null)}>{tr("Prüfformular ohne Modell","Review form without a model")}</button>
      <button type="button" disabled={busy} onClick={()=>void Promise.all([page.refetch(),recovery.query.refetch(),selected?entry.refetch():Promise.resolve(),selected&&versionId?version.refetch():Promise.resolve()])}>{t("workflowRefresh")}</button>
      <button type="button" disabled={busy||!!edit||!!recovery.operation} onClick={()=>void productRequest(client,`/projects/${project}/workflow-journal/archive`,"POST",{before_ms:Date.now(),keep_snapshots:20}).then(()=>recovery.query.refetch()).catch(setError)}>Bestätigte Journalhistorie archivieren</button></div>
    <WorkflowStarterImport providers={providers} language={language} disabled={!recovery.ready||busy||blocked||!!edit||frozen} onCreate={definition=>newEditor(definition,null)} summary={definition=><WorkflowDefinitionSummary definition={definition} providers={providers} t={t}/>}/>
    <WorkflowExtractionStarter providers={providers} language={language} disabled={!recovery.ready||busy||blocked||!!edit||frozen} onCreate={definition=>newEditor(definition,null)}/>
    <WorkflowOperationPanel operation={recovery.operation} busy={busy} error={recovery.error} t={t} commit={()=>void write("save")} discard={()=>void recovery.action("discard")} acknowledge={()=>void acknowledge()}>
      {recovery.operation&&(recovery.operation.request.kind==="create"||recovery.operation.request.kind==="publish")&&<WorkflowDefinitionSummary definition={recovery.operation.request.request.definition} providers={providers} t={t}/>}
    </WorkflowOperationPanel>
    {recovery.operation&&!recovery.uncertainPrepare&&<WorkflowSnapshotRecovery key={recovery.operation.id} client={client} project={project} operation={recovery.operation} busy={busy} blocked={blocked} copied={!!restoredCopy&&restoredCopy.operation_id===recovery.operation.id} t={t} onCopy={copySnapshot} onExport={exportDraft} summary={definition=><WorkflowDefinitionSummary definition={definition} providers={providers} t={t}/>}/>}
    {recovery.uncertainPrepare&&<p role="status">{t("workflowJournalPreparePending")} <button disabled={busy} onClick={()=>void write("save")}>{t("workflowRetrySame")}</button></p>}
    {page.isPending&&<p role="status">{t("loading")}</p>}{page.isError&&<p role="alert">{t("workflow_load_failed")}</p>}
    {!page.isPending&&!page.isError&&!page.data?.items.length&&<p>{t("workflowEmpty")}</p>}
    <div className="workflow-library-layout"><nav className="workflow-list" aria-label={t("workflowLibrary")}>
      <h3>{t("workflowDrafts")}</h3>{recovery.query.data?.drafts.filter(draft=>!draft.content.archived).map(draft=>{const data=restoreWorkflowEditor(draft.content);return <button type="button" key={draft.id} disabled={busy||blocked||frozen} onClick={()=>openDraft(draft)}><span>{data?.name||t("workflowDraftUntitled")}<small>{t("workflowDraftSaved")}</small></span></button>;})}
      {page.data?.items.map(item=><button type="button" key={item.id} disabled={busy||blocked||frozen||!!edit} onClick={()=>choose(item)} aria-current={selected===item.id?"page":undefined}><Layers size={17}/><span><strong>{item.name}</strong><small>{t("workflowVersion")} {item.versions.at(-1)?.revision??"—"}{item.revoked?" · "+t("workflowRevoked"):""}</small></span></button>)}
      {!!page.data?.total&&<div className="workflow-pagination"><button type="button" disabled={offset===0||busy||blocked||frozen} onClick={()=>setOffset(Math.max(0,offset-30))}>{t("workflowPrevious")}</button><button type="button" disabled={page.data.next_offset==null||busy||blocked||frozen} onClick={()=>setOffset(page.data!.next_offset!)}>{t("workflowNext")}</button></div>}</nav>
      <div className="workflow-library-content">{edit?<><p role="status">{t(saving?"saving":dirty?"unsaved":"workflowDraftSaved")}</p>
        <WorkflowEditor key={edit.id} client={client} project={project} value={edit.definition} setValue={definition=>{setAutosavePaused(false);setEdit({...edit,definition});}} providers={providers} disabled={busy||editorFrozen||!!draftConflict||!!saveRequest.current} t={t} onSubmit={event=>void write("save",event)}/>
        <button type="button" disabled={busy||editorFrozen||!!draftConflict||(!dirty&&!saveRequest.current)} onClick={()=>void save().catch(()=>{})}>{t(saveRequest.current?"workflowRetrySame":"workflowDraftSave")}</button>
        {(!!error||saveRequest.current)&&<div className="actions">
          <button type="button" disabled={busy} onClick={()=>void exportDraft({editor_format:workflowEditorFormat,content:draftContent(edit),unconfirmed_request:saveRequest.current})}>{t("workflowDraftExport")}</button>
          {saveRequest.current&&error instanceof RequestError&&["invalid_request","budget_exceeded"].includes(error.details.code)&&<button type="button" disabled={busy||editorFrozen} onClick={()=>{saveRequest.current=null;setAutosavePaused(true);setError(undefined);}}>{t("workflowDraftEditRejected")}</button>}
          {edit.revision>0&&(returnConfirm?<><p>{t("workflowDraftReturnHint")}</p><button type="button" disabled={busy||frozen} onClick={()=>void returnToConfirmed()}>{t("workflowDraftReturnConfirm")}</button><button type="button" disabled={busy} onClick={()=>setReturnConfirm(false)}>{t("cancel")}</button></>:<button type="button" disabled={busy||frozen} onClick={()=>setReturnConfirm(true)}>{t("workflowDraftReturn")}</button>)}
        </div>}
        {ownCopy&&<p role="status">{t("workflowSnapshotCopyHint")}</p>}
        {ownCopy&&edit.origin&&<button type="button" disabled={busy||blocked||!recovery.ready} onClick={()=>void choosePublicationBase()}>{t("workflowSnapshotChooseBase")}</button>}
        {snapshots.data&&<details><summary>{t("workflowDraftCopies")}</summary>{snapshots.data.items.filter(snapshot=>snapshot.conflict).map(snapshot=><button type="button" key={snapshot.id} disabled={busy||blocked||frozen} onClick={()=>void client.workflowDraftSnapshot(project,edit.id,snapshot.id).then(value=>{const definition=restoreWorkflowEditor(value.content);if(definition)newEditor(definition,value.content.origin??null);}).catch(setError)}>{t("workflowDraftConflictCopy")} · {new Date(snapshot.created_at).toLocaleString()}</button>)}
          <div className="actions"><button disabled={snapshotOffset===0||busy} onClick={()=>setSnapshotOffset(Math.max(0,snapshotOffset-20))}>{t("workflowPrevious")}</button><button disabled={snapshots.data.next_offset==null||busy} onClick={()=>setSnapshotOffset(snapshots.data!.next_offset!)}>{t("workflowNext")}</button></div></details>}
      </>:unknownDraft?<section><p role="alert">{t("workflowDraftUnsupported")}</p><button disabled={busy} onClick={()=>void exportDraft(unknownDraft)}>{t("workflowDraftExport")}</button><details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(unknownDraft,null,2)}</pre></details></section>:selected&&<>
        {entry.isPending||version.isPending?<p role="status">{t("loading")}</p>:null}{entry.isError||version.isError?<p role="alert">{t("workflowUnavailable")}</p>:null}
        {entry.data&&<><h3>{entry.data.name}</h3>{entry.data.revoked&&<p role="status">{t("workflowRevoked")}</p>}<label>{t("workflowChooseVersion")}<select value={versionId} disabled={busy||frozen} onChange={event=>setVersionId(event.target.value)}>{entry.data.versions.slice().reverse().map(ref=><option key={ref.id} value={ref.id}>{t("workflowVersion")} {ref.revision} · {ref.name}</option>)}</select></label></>}
        {version.data&&<><WorkflowDefinitionSummary definition={version.data.definition} providers={providers} t={t}/><p>{t("workflowImmutableHint")}</p><div className="actions">
          <button type="button" disabled={!recovery.ready||busy||!entry.data||entry.data.revoked||!editableWorkflow(version.data.definition)} onClick={()=>newEditor(structuredClone(version.data!.definition),{workflow_id:entry.data!.id,version_id:version.data!.reference.id,base_revision:entry.data!.revision})}>{t("workflowEditVersion")}</button>
          {!entry.data?.revoked&&<button type="button" disabled={!recovery.ready||busy} onClick={()=>setRevokeConfirm(true)}>{t("workflowRevoke")}</button>}</div>
          {!editableWorkflow(version.data.definition)&&<p role="alert">{t("workflowReadOnlyDefinition")}</p>}<details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(version.data,null,2)}</pre></details></>}
      </>}
      {draftConflict&&<section className="conflict" role="alert"><p>{t("workflowDraftConflict")}</p>
        <p>{t("workflowDraftConflictPreserved")}</p><details><summary>{t("workflowDraftServer")}</summary>{restoreWorkflowEditor(draftConflict.draft.content)?<WorkflowDefinitionSummary definition={restoreWorkflowEditor(draftConflict.draft.content)!} providers={providers} t={t}/>:<p>{t("workflowDraftUnsupported")}</p>}</details>
        <div className="actions"><button disabled={busy||frozen} onClick={()=>openDraft(draftConflict.draft)}>{t("workflowDraftUseServer")}</button>
          <button disabled={busy||frozen} onClick={()=>{const definition=restoreWorkflowEditor(draftConflict.snapshot.content);if(definition){setDraftConflict(null);setEdit({id:crypto.randomUUID(),revision:0,origin:draftConflict.snapshot.content.origin??null,definition,saved:"",snapshot:null,sha256:"",archived:false});setError(undefined);}}}>{t("workflowDraftKeepCopy")}</button></div></section>}
      {conflicting&&<section className="conflict" role="alert"><p>{t("workflowConflict")}</p>{currentConflictVersion.data&&<WorkflowDefinitionSummary definition={currentConflictVersion.data.definition} providers={providers} t={t}/>}
        <button type="button" disabled={busy||frozen||!currentConflictVersion.data} onClick={()=>{if(edit)setEdit({...edit,origin:{workflow_id:conflicting.id,version_id:currentConflictVersion.data!.reference.id,base_revision:conflicting.revision}});setConflicting(null);setError(undefined);}}>{t("workflowRebase")}</button></section>}
      {revokeConfirm&&!frozen&&<section className="banner"><p>{t("workflowRevokeHint")}</p><div className="actions"><button type="button" disabled={busy} onClick={()=>setRevokeConfirm(false)}>{t("cancel")}</button><button type="button" disabled={busy} onClick={()=>void write("revoke")}>{t("workflowRevokeConfirm")}</button></div></section>}
      {!!error&&<div role="alert">{t(error instanceof RequestError&&error.details.code==="budget_exceeded"?"workflowJournalQuota":error instanceof RequestError&&error.details.code==="invalid_request"?"workflowDraftInputLimit":error instanceof Error&&error.message==="workflowDraftConflict"?"workflowDraftConflict":error instanceof Error&&error.message==="workflowInvalid"?"workflowInvalid":"workflowDraftSaveFailed")}<details><summary>{t("diagnostics")}</summary><p>{error instanceof Error?error.message:String(error)}</p></details></div>}
      {!recovery.operation&&!!recovery.error&&<p role="alert">{t("workflowJournalUnavailable")}</p>}
      {edit&&<div className="actions"><button type="button" disabled={busy||blocked||frozen} onClick={()=>{setRestoredCopy(null);setEdit(null);setError(undefined);setConflicting(null);}}>{t("workflowDraftClose")}</button>
        <button type="submit" form="workflow-editor" className="primary" disabled={busy||frozen||!!draftConflict||!!conflicting||!recovery.ready}>{t(edit.origin?"workflowPublish":"workflowSave")}</button></div>}
    </div></div>
  </section>;
}

export function WorkflowDefinitionSummary({definition,providers,t,depth=0}:{definition:WorkflowDefinition;providers:ProviderProfile[];t:T;depth?:number}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const budgets=completeWorkflowBudgets(definition.budgets);
  const toolName=(tool:string)=>tool==="create_artifact"?t("workflowCreateArtifact"):tool==="list_artifacts"?t("workflowListArtifacts"):tool==="read_artifact"?t("workflowReadArtifact"):tool==="request_input"?t("workflowRequestInput"):tool;
  return <section className="workflow-summary"><p>{t(definition.mode.mode==="sequential"?"workflowSequential":definition.mode.mode==="parallel"?"workflowParallel":"workflowHierarchical")}{definition.mode.mode!=="sequential"?" · "+t("workflowMaxParallel")+": "+definition.mode.max_parallel:""}</p>
    <p>{t(definition.failure==="stop"?"workflowStop":"workflowContinue")}</p><p>{t("workflowRights")}: {definition.rights.tools.length?definition.rights.tools.map(toolName).join(", "):t("workflowNoTools")}</p>
    <dl><dt>{t("maxRunSteps")}</dt><dd>{budgets.max_steps}</dd><dt>{t("maxRunSeconds")}</dt><dd>{budgets.max_seconds}</dd><dt>{t("maxReplyTokens")}</dt><dd>{budgets.max_output_tokens}</dd><dt>{t("maxRunTokens")}</dt><dd>{budgets.max_total_tokens}</dd></dl>
    <WorkflowWorkspaceNotice value={definition.rights}/><h4>{t("workflowRoles")}</h4><ul>{definition.roles.map(role=>{const profile=providers.find(item=>item.id===role.provider_id&&item.revision===role.provider_revision);return <li key={role.id}><strong>{role.id}</strong><p>{profile?profile.name+" · "+profile.model:t("workflowProviderChanged")}</p>{profile&&<p>{profile.endpoint}</p>}<p>{role.instructions}</p><WorkflowWorkspaceNotice value={role.rights}/><p>{t("workflowRights")}: {role.rights.tools.length?role.rights.tools.map(toolName).join(", "):t("workflowNoTools")}</p></li>;})}</ul>
    <h4>{t("workflowSteps")}</h4><ol>{definition.steps.map(step=><li key={step.id}><strong>{step.role_id}</strong><p>{step.goal}</p><WorkflowWorkspaceNotice value={step.rights}/>{step.execution&&<p><code>{JSON.stringify(step.execution)}</code></p>}{step.zero_context&&<p>zero_context: true</p>}{!!step.dependencies.length&&<p>{t("workflowDependencies")}: {step.dependencies.map(id=>{const nodes=workflowDataNodes(definition),node=nodes.find(item=>item.id===id);return node?`${nodes.indexOf(node)+1}. ${node.goal}`:t("workflowUnavailable");}).join("; ")}</p>}<p>{t("workflowRights")}: {step.rights.tools.length?step.rights.tools.map(toolName).join(", "):t("workflowNoTools")}</p></li>)}</ol>{Object.entries(definition.documents??{}).map(([id,step])=><section className="product-item" key={id}><h4>{step.name}</h4><p>{step.operation.kind==="read_text"?tr("Text lesen","Read text"):step.operation.kind==="preview"?tr("Seitenvorschau","Page preview"):step.operation.kind==="ocr"?tr("Scans erkennen (OCR)","Recognize scans (OCR)"):step.operation.kind==="append_table_rows"?tr("Excel-Tabelle ergänzen","Append to Excel table"):tr("Dokument erstellen","Create document")}{step.operation.kind==="create"?" · "+step.operation.format.toUpperCase():""}</p><WorkflowWorkspaceNotice value={step.rights}/><p>{tr("Ausführung ohne Modellkonto; ausdrückliche Dokumentengine und bestätigte Arbeitskopie erforderlich.","No model account; explicit document engine and confirmed working copy required.")}</p></section>)}<WorkflowDataflowSummary definition={definition}/>{depth<4&&Object.entries(definition.subflows??{}).map(([id,call])=><details key={id}><summary>{tr("Unterablauf","Subflow")}: {call.name}</summary><p>{tr("Eingefrorene Kopie","Frozen copy")}: {call.definition.name}</p><WorkflowWorkspaceNotice value={call.rights}/><p>{tr("Rückgabe","Return")}: {call.result.step_id}{call.result.pointer}</p><WorkflowDefinitionSummary definition={call.definition} providers={providers} t={t} depth={depth+1}/></details>)}{depth<4&&Object.entries(definition.iterations??{}).map(([id,loop])=><details key={id}><summary>{tr("Listenablauf","List workflow")}: {loop.name}</summary><p>{tr("Vorlage für Listeneinträge","Entry template")}: {loop.definition.name}</p><WorkflowWorkspaceNotice value={loop.rights}/><p>{tr("Gleichzeitig aktive Einträge","Concurrent active entries")}: {loop.max_parallel}</p><p>{tr("Rückgabe pro Eintrag","Return per entry")}: {loop.result.step_id}{loop.result.pointer}</p><ul>{Object.entries(loop.arguments).map(([field,binding])=><li key={field}><strong>{field}</strong>: {binding.source==="item"?tr("Aktueller Listeneintrag","Current list entry"):binding.source==="index"?tr("Position in der Liste","Position in the list"):binding.source==="input"?tr("Gemeinsame Listeneingabe","Shared list input"):tr("Fester Wert","Constant value")}{"pointer" in binding?` ${binding.pointer||"/"}`:binding.source==="literal"?` ${JSON.stringify(binding.value)}`:""}</li>)}</ul><p>{tr("Gemeinsame Obergrenzen dieser Liste","Shared limits for this list")}: {loop.budgets.max_steps} {tr("Schritte","steps")}, {loop.budgets.max_seconds} s, {loop.budgets.max_total_tokens} {tr("Token","tokens")}</p><WorkflowDefinitionSummary definition={loop.definition} providers={providers} t={t} depth={depth+1}/></details>)}</section>;
}
function ToolRights({tools,allowed,onChange,t}:{tools:string[];allowed:readonly string[];onChange:(tools:string[])=>void;t:T}) {
  return <fieldset className="workflow-tools"><legend>{t("workflowRights")}</legend>{workflowTools.map(tool=><label key={tool}><input type="checkbox" disabled={!allowed.includes(tool)&&!tools.includes(tool)} checked={tools.includes(tool)} onChange={event=>onChange(event.target.checked?[...tools.filter(value=>value!==tool),tool]:tools.filter(value=>value!==tool))}/><span>{toolKeys[tool]?t(toolKeys[tool]!):tool}</span></label>)}{!tools.length&&<p className="muted">{t("workflowNoTools")}</p>}</fieldset>;
}
function BudgetEditor({value,onChange,title,t,context}:{value:Budgets;onChange:(value:Budgets)=>void;title:Key;t:T;context?:number}) {
  return <section className="workflow-budget"><h4>{t(title)}</h4><RunBudgets value={completeWorkflowBudgets(value)} setValue={onChange} disabled={false} t={t} contextTokens={context}/></section>;
}
function WorkflowRoleLibraryPicker({client,definition,index,providers,disabled,onChange}:{client:Client;definition:WorkflowDefinition;index:number;providers:ProviderProfile[];disabled:boolean;onChange:(value:WorkflowDefinition)=>void}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b;
  const [personality,setPersonality]=useState<Personality|null>(null),[agent,setAgent]=useState<AgentTemplate|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const qc=useQueryClient(),library=useAgentLibrary(client),role=definition.roles[index],source=role.agent_config?.agent_source??role.agent_config?.personality_source,isAgent=!!role.agent_config?.agent_source;
  const latest=isAgent?library.agents.data?.find(item=>item.id===source?.id):library.personalities.data?.find(item=>item.id===source?.id);
  const current=useRef(definition);current.current=definition;
  async function apply(){if(disabled||busy||!agent&&!personality)return;setBusy(true);setError("");try{
    const source=definition,bound=await bindCodexLibraryModel({client},agent??personality!),available=bound.profile?[...providers.filter(p=>p.id!==bound.profile!.id),bound.profile]:providers;const next=agent?await applyAgentToWorkflow(source,index,bound.item as AgentTemplate,available):{...source,roles:source.roles.map((role,i)=>i===index?applyPersonalityToWorkflow(role,bound.item as Personality,available):role)};if(bound.profile)await qc.invalidateQueries({queryKey:["providers"]});
    if(current.current!==source)throw new Error(tr("Der Workflow wurde geändert. Vorlage erneut anwenden.","The workflow changed. Apply the template again."));
    onChange(next);setAgent(null);setPersonality(null);
  }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}}
  async function refresh(){if(disabled||busy||!source)return;const original=definition;setBusy(true);setError("");try{const result=await (isAgent?library.agents.refetch():library.personalities.refetch());if(result.error)throw result.error;const item=result.data?.find(entry=>entry.id===source.id);if(!item)throw new Error(tr("Die ursprüngliche Bibliotheksvorlage ist nicht mehr verfügbar.","The original library template is no longer available."));const bound=await bindCodexLibraryModel({client},item),available=bound.profile?[...providers.filter(p=>p.id!==bound.profile!.id),bound.profile]:providers;const next=await refreshWorkflowRoleFromLibrary(original,index,bound.item,available);if(bound.profile)await qc.invalidateQueries({queryKey:["providers"]});if(current.current!==original)throw new Error(tr("Der Workflow wurde geändert. Aktualisierung erneut starten.","The workflow changed. Refresh again."));onChange(next);}catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}}
  return <details><summary>{tr("Aus der Agentbibliothek übernehmen","Apply from agent library")}</summary>{source&&<div><p>{latest?.value.name??source.id} · {tr("Kopierte Version","Copied version")} {source.revision}{latest&&latest.revision!==source.revision?` → ${latest.revision}`:""}</p><button type="button" disabled={disabled||busy} onClick={()=>void refresh()}>{tr("Aus Vorlage aktualisieren","Update from template")}</button><small>{tr("Übernimmt die aktuelle Bibliotheksversion ausdrücklich in den Entwurf. Rollen-ID, aktuelle Rechte- und Budgetgrenzen bleiben erhalten.","Explicitly copies the current library version into this draft. The role ID and current permission and budget limits are retained.")}</small></div>}<AgentLibrarySelector client={client} disabled={disabled||busy} personalityId={personality?.id} agentId={agent?.id} onPersonality={value=>{setPersonality(value);setAgent(null);}} onAgent={value=>{setAgent(value);setPersonality(null);}}/><button type="button" disabled={disabled||busy||!agent&&!personality} onClick={()=>void apply()}>{tr("Auf diese Rolle anwenden","Apply to this role")}</button><p className="muted">{tr("Die Rolle erhält eine Kopie. Werkzeugrechte und Budgets bleiben innerhalb der Workflowgrenzen.","The role receives a snapshot. Tool permissions and budgets stay within the workflow limits.")}</p>{error&&<p role="alert">{error}</p>}</details>;
}
export function WorkflowEditor({client,project,value,setValue:onSetValue,providers,disabled,t,onSubmit}:{client?:Client;project?:string;value:WorkflowDefinition;setValue:(value:WorkflowDefinition)=>void;providers:ProviderProfile[];disabled:boolean;t:T;onSubmit:(event:FormEvent)=>void}) {
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en;
  function setValue(next:WorkflowDefinition){onSetValue(reconcileWorkflowDataflow(next,value));}
  const formId=useId();const [roleCollision,setRoleCollision]=useState<number|null>(null);
  function roleChange(index:number,patch:Partial<WorkflowDefinition["roles"][number]>) {setValue({...value,roles:value.roles.map((role,i)=>i===index?{...role,...patch}:role)});}
  function stepChange(index:number,patch:Partial<WorkflowDefinition["steps"][number]>) {setValue({...value,steps:value.steps.map((step,i)=>i===index?{...step,...patch}:step)});}
  const stale=value.roles.some(role=>role.provider_id&&!providers.some(provider=>provider.id===role.provider_id&&provider.revision===role.provider_revision));
  return <form id="workflow-editor" className="workflow-editor" onSubmit={onSubmit}><fieldset disabled={disabled}><label>{t("workflowName")}<input value={value.name} required maxLength={200} onChange={event=>setValue({...value,name:event.target.value})}/></label>
    <div className="workflow-fields"><label>{t("workflowMode")}<select value={value.mode.mode} onChange={event=>setValue({...value,mode:event.target.value==="parallel"?{mode:"parallel",max_parallel:2}:event.target.value==="hierarchical"?{mode:"hierarchical",max_parallel:2,max_depth:1,max_delegations:1,manager_step_id:value.steps.at(-1)?.id??""}:{mode:"sequential"}})}><option value="sequential">{t("workflowSequential")}</option><option value="parallel">{t("workflowParallel")}</option><option value="hierarchical">{t("workflowHierarchical")}</option></select></label>
      {value.mode.mode!=="sequential"&&<label>{t("workflowMaxParallel")}<input type="number" min={1} max={32} step={1} value={value.mode.max_parallel} onChange={event=>{if(value.mode.mode!=="sequential")setValue({...value,mode:{...value.mode,max_parallel:Number(event.target.value)}});}} required/></label>}
      {value.mode.mode==="hierarchical"&&<><label>{t("workflowManagerStep")}<select value={value.mode.manager_step_id??""} onChange={event=>{if(value.mode.mode==="hierarchical")setValue({...value,mode:{...value.mode,manager_step_id:event.target.value||undefined}});}}><option value="">Dynamische Delegation durch konfigurierte Rollen</option>{value.steps.map(step=><option key={step.id} value={step.id}>{step.role_id} · {step.goal||step.id}</option>)}</select><small>{t("workflowManagerHint")}</small></label><label>Delegationstiefe<input type="number" min={1} max={8} value={value.mode.max_depth} onChange={e=>{if(value.mode.mode==="hierarchical")setValue({...value,mode:{...value.mode,max_depth:Number(e.target.value)}});}}/></label><label>Maximale Delegationen<input type="number" min={1} max={64} value={value.mode.max_delegations} onChange={e=>{if(value.mode.mode==="hierarchical")setValue({...value,mode:{...value.mode,max_delegations:Number(e.target.value)}});}}/></label></>}
      <label>{t("workflowFailure")}<select value={value.failure} onChange={event=>setValue({...value,failure:event.target.value==="stop"?"stop":"continue_independent"})}><option value="continue_independent">{t("workflowContinue")}</option><option value="stop">{t("workflowStop")}</option></select></label></div>
    <p className="muted">{t("workflowRightsHint")}</p><ToolRights tools={value.rights.tools} allowed={workflowTools} onChange={tools=>setValue({...value,rights:{...value.rights,tools},roles:value.roles.map(role=>({...role,rights:{...role.rights,tools:role.rights.tools.filter(tool=>tools.includes(tool))}})),steps:value.steps.map(step=>({...step,rights:{...step.rights,tools:step.rights.tools.filter(tool=>tools.includes(tool))}}))})} t={t}/>
    <WorkflowWorkspaceRight value={value.rights} onChange={rights=>setValue({...value,rights})}/><label>Ordnerrechte (ausgewählte Ordner-IDs, mit Kommas getrennt)<input value={value.rights.folders.join(", ")} onChange={e=>setValue({...value,rights:{...value.rights,folders:e.target.value.split(",").map(x=>x.trim()).filter(Boolean)}})}/></label>
    <label>MCP-Werkzeugrechte (ausgewählte mcp_…-Aliase)<input value={value.rights.tools.filter(x=>x.startsWith("mcp_")).join(", ")} onChange={e=>setValue({...value,rights:{...value.rights,tools:[...value.rights.tools.filter(x=>!x.startsWith("mcp_")),...e.target.value.split(",").map(x=>x.trim()).filter(Boolean)]}})}/></label>
    <p>{t("workflowBudgetHint")}</p><BudgetEditor value={value.budgets} onChange={budgets=>setValue({...value,budgets})} title="workflowGlobalBudget" t={t}/>
    <h3>{t("workflowRoles")}</h3>{stale&&<p role="alert">{t("workflowProviderChanged")}</p>}
    {value.roles.map((role,index)=><section className="workflow-editor-card" key={index}><h4>{t("workflowStepRole")} {index+1}</h4><label htmlFor={`${formId}-role-${index}`}>{t("workflowRoleName")}<input id={`${formId}-role-${index}`} value={role.id} required pattern={workflowRolePattern} maxLength={64} onChange={event=>{const result=renameWorkflowRole(value,index,event.target.value);setRoleCollision(result.collision?index:null);if(!result.collision)setValue(result.definition);}}/></label><p className="muted">{t("workflowRoleIdHint")}</p>{roleCollision===index&&<p role="alert">{t("workflowRoleCollision")}</p>}
      {client&&<WorkflowRoleLibraryPicker client={client} definition={value} index={index} providers={providers} disabled={disabled} onChange={setValue}/>}
      <label>{t("workflowInstructions")}<textarea required value={role.instructions} rows={3} maxLength={256000} aria-invalid={!workflowRoleInstructionsValid(role.instructions)} onChange={event=>roleChange(index,{instructions:event.target.value,agent_config:role.agent_config?{...role.agent_config,instructions:event.target.value}:undefined})}/></label>
      <label>{t("chooseProvider")}<select required value={providers.some(p=>p.id===role.provider_id&&p.revision===role.provider_revision)?role.provider_id:""} onChange={event=>{const profile=providers.find(p=>p.id===event.target.value);if(profile)roleChange(index,{provider_id:profile.id,provider_revision:profile.revision,model_binding:undefined});}}><option value="">{t("chooseProvider")}</option>{providers.map(profile=><option key={profile.id} value={profile.id}>{profile.name} · {profile.model}</option>)}</select></label>
      <StructuredOutputEditor value={role.output} onChange={output=>roleChange(index,{output})}/>
      {client&&<ModelRegistryPicker client={client} providerId={role.provider_id} providerRevision={role.provider_revision} value={role.model_binding??undefined} onChange={model_binding=>roleChange(index,{model_binding})} disabled={disabled}/>}
      <ToolRights tools={role.rights.tools} allowed={value.rights.tools} onChange={tools=>roleChange(index,{rights:{...role.rights,tools}})} t={t}/>
      <WorkflowWorkspaceRight value={role.rights} available={!!value.rights.current_workspace} label={tr("Aktuellen Arbeitsbereich für diese Rolle erlauben","Allow current workspace for this role")} onChange={rights=>roleChange(index,{rights})}/><label>Ordnerrechte dieser Rolle<input value={role.rights.folders.join(", ")} onChange={e=>roleChange(index,{rights:{...role.rights,folders:e.target.value.split(",").map(x=>x.trim()).filter(Boolean)}})}/></label>
      <label>MCP-Aliase dieser Rolle<input value={role.rights.tools.filter(x=>x.startsWith("mcp_")).join(", ")} onChange={e=>roleChange(index,{rights:{...role.rights,tools:[...role.rights.tools.filter(x=>!x.startsWith("mcp_")),...e.target.value.split(",").map(x=>x.trim()).filter(Boolean)]}})}/></label>
      {value.mode.mode==="hierarchical"&&<fieldset><legend>Delegation an Rollen</legend>{value.roles.map(target=><label key={target.id}><input type="checkbox" checked={role.delegate_to.includes(target.id)} onChange={e=>roleChange(index,{delegate_to:e.target.checked?[...role.delegate_to,target.id]:role.delegate_to.filter(x=>x!==target.id)})}/>{target.id}</label>)}<small>Die delegierende Rolle und ihr Schritt benötigen delegate_agents. Kinder erhalten nur die Schnittmenge der erlaubten Rechte.</small></fieldset>}
      <BudgetEditor value={role.budgets} onChange={budgets=>roleChange(index,{budgets})} title="workflowRoleBudget" t={t} context={providers.find(p=>p.id===role.provider_id)?.context_tokens}/>
      <button type="button" disabled={(value.roles.length===1&&value.steps.length>0)||value.steps.some(step=>step.role_id===role.id)} onClick={()=>setValue({...value,roles:value.roles.filter((_,i)=>i!==index)})}>{t("workflowRemoveRole")}</button></section>)}
    <button type="button" disabled={value.roles.length>=32} onClick={()=>{const id="Agent_"+crypto.randomUUID().slice(0,6);setValue({...value,roles:[...value.roles,{id,instructions:t("workflowDefaultInstructions"),provider_id:"",provider_revision:0,rights:{tools:[],folders:[]},budgets:{...value.budgets},delegate_to:[]}]});}}><Plus size={16}/>{t("workflowAddRole")}</button>
    <h3>{t("workflowSteps")}</h3>{value.steps.some(step=>value.roles.filter(role=>role.id===step.role_id).length!==1)&&<p role="alert">{t("workflowRoleBindingRequired")}</p>}{value.mode.mode==="sequential"&&<p>{t("workflowSequentialHint")}</p>}
    {value.steps.map((step,index)=><section className="workflow-editor-card" key={step.id}><h4>{t("steps")} {index+1}</h4><label className="product-check"><input type="checkbox" checked={step.zero_context??false} onChange={event=>stepChange(index,{zero_context:event.target.checked||undefined})}/>{tr("Frühere Schrittresultate aus dem Kontext ausschließen","Exclude earlier step results from context")}</label><label>{tr("Schrittausführung","Step execution")}<select value={step.execution?.kind??"model"} onChange={event=>stepChange(index,{execution:event.target.value==="web_fetch"?{kind:"web_fetch",url:""}:event.target.value==="web_search"?{kind:"web_search",query:""}:undefined})}><option value="model">{tr("Modell","Model")}</option><option value="web_fetch">Web fetch · Browser</option><option value="web_search">Web search · Browser</option></select></label>{step.execution?.kind==="web_fetch"&&<label>URL<input type="url" required maxLength={8192} value={step.execution.url} onChange={event=>stepChange(index,{execution:{kind:"web_fetch",url:event.target.value}})}/></label>}{step.execution?.kind==="web_search"&&<label>{tr("Suchanfrage","Search query")}<input required maxLength={8000} value={step.execution.query} onChange={event=>stepChange(index,{execution:{kind:"web_search",query:event.target.value}})}/></label>}{step.execution&&<p className="muted">{tr("Benötigt Browserrechte für Workflow, Rolle, Schritt und Lauf. Jede konkrete Browseraktion benötigt eine Freigabe.","Requires Browser rights for the workflow, role, step, and run. Each exact browser action requires approval.")}</p>}<label>{t("workflowGoal")}<textarea required rows={3} value={step.goal} maxLength={16000} onChange={event=>stepChange(index,{goal:event.target.value})}/></label>
      <label>{t("workflowStepRole")}<select value={step.role_id} required onChange={event=>stepChange(index,{role_id:event.target.value})}>{value.roles.filter(role=>role.id===step.role_id).length!==1&&<option value={step.role_id} disabled>{t("workflowRoleBindingChoose")}</option>}{value.roles.filter(role=>role.id&&value.roles.filter(other=>other.id===role.id).length===1).map((role,i)=><option key={i} value={role.id}>{role.id}</option>)}</select></label>
      {value.mode.mode!=="sequential"&&<fieldset className="workflow-dependencies"><legend>{t("workflowDependencies")}</legend>{workflowDataNodes(value).filter(candidate=>candidate.id!==step.id).map(candidate=><label key={candidate.id}><input type="checkbox" checked={step.dependencies.includes(candidate.id)} onChange={event=>stepChange(index,{dependencies:event.target.checked?[...step.dependencies,candidate.id]:step.dependencies.filter(id=>id!==candidate.id)})}/><span>{workflowDataNodes(value).findIndex(item=>item.id===candidate.id)+1}. {candidate.goal||t("workflowGoal")}</span></label>)}{!step.dependencies.length&&<p className="muted">{t("workflowNoDependencies")}</p>}</fieldset>}
      <ToolRights tools={step.rights.tools} allowed={value.rights.tools.filter(tool=>value.roles.find(role=>role.id===step.role_id)?.rights.tools.includes(tool))} onChange={tools=>stepChange(index,{rights:{...step.rights,tools}})} t={t}/>
      <WorkflowWorkspaceRight value={step.rights} available={!!value.rights.current_workspace&&!!value.roles.find(role=>role.id===step.role_id)?.rights.current_workspace} label={tr("Aktuellen Arbeitsbereich für diesen Schritt erlauben","Allow current workspace for this step")} onChange={rights=>stepChange(index,{rights})}/><label>Ordnerrechte dieses Schritts<input value={step.rights.folders.join(", ")} onChange={e=>stepChange(index,{rights:{...step.rights,folders:e.target.value.split(",").map(x=>x.trim()).filter(Boolean)}})}/></label>
      <label>MCP-Aliase dieses Schritts<input value={step.rights.tools.filter(x=>x.startsWith("mcp_")).join(", ")} onChange={e=>stepChange(index,{rights:{...step.rights,tools:[...step.rights.tools.filter(x=>!x.startsWith("mcp_")),...e.target.value.split(",").map(x=>x.trim()).filter(Boolean)]}})}/></label>
      <BudgetEditor value={step.budgets} onChange={budgets=>stepChange(index,{budgets})} title="workflowStepBudget" t={t}/>
      <div className="workflow-toolbar"><button type="button" disabled={index===0} aria-label={t("workflowMoveUp")+" · "+(index+1)} onClick={()=>{const steps=[...value.steps];[steps[index-1],steps[index]]=[steps[index],steps[index-1]];setValue({...value,steps});}}><ArrowUp size={16}/>{t("workflowMoveUp")}</button><button type="button" disabled={index===value.steps.length-1} aria-label={t("workflowMoveDown")+" · "+(index+1)} onClick={()=>{const steps=[...value.steps];[steps[index+1],steps[index]]=[steps[index],steps[index+1]];setValue({...value,steps});}}><ArrowDown size={16}/>{t("workflowMoveDown")}</button>
        <button type="button" disabled={value.steps.length===1&&Object.keys(value.documents??{}).length===0&&Object.keys(value.controls??{}).length===0&&Object.keys(value.subflows??{}).length===0&&Object.keys(value.iterations??{}).length===0} onClick={()=>setValue(removeWorkflowNode(value,step.id))}>{t("workflowRemoveStep")}</button></div></section>)}
    <button type="button" disabled={workflowDataNodes(value).length>=64||value.roles.length===0} onClick={()=>{const role=value.roles[0];if(role)setValue({...value,steps:[...value.steps,{id:"step-"+crypto.randomUUID().slice(0,8),role_id:role.id,goal:"",dependencies:[],rights:{tools:[],folders:[]},budgets:{...value.budgets}}]});}}><Plus size={16}/>{t("workflowAddStep")}</button>
    {client&&project&&<WorkflowSubflowsEditor client={client} project={project} value={value} onChange={setValue}/>}
    {client&&project&&<WorkflowIterationsEditor client={client} project={project} value={value} onChange={setValue}/>}
    <WorkflowControlsEditor value={value} onChange={setValue}/>
    <WorkflowDocumentsEditor value={value} onChange={setValue}/>
    <WorkflowDataflowEditor value={value} onChange={setValue}/>
  </fieldset></form>;
}
