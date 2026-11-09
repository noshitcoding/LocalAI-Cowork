import {useEffect,useState} from "react";
import {Client,decode,type PrepareOwnerExecutionPlanV2,type RunRecord,type RunRequest,type WorkflowDefinition} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {SnapshotExecutionPlanReview} from "./SnapshotExecutionPlanReview";
import {workflowDataError} from "./workflowDataflow";
import {useView} from "./view";
import {defaultExecutionTarget} from "./executionDefaults";

type Props={client:Client;request:RunRequest;onStarted:(run:RunRecord)=>Promise<void>;onRetired:()=>void};
const documentTarget:PrepareOwnerExecutionPlanV2["target"]={kind:"document_engine"};

/** Read the published source, including frozen nested templates; never infer process consent. */
export function workflowRequiresDocumentEngine(definition:WorkflowDefinition,depth=0):boolean{
  if(depth>4)throw new Error("workflow_definition_unsupported");
  const calls=[...Object.values(definition.subflows??{}),...Object.values(definition.iterations??{})];
  return [definition.rights,...definition.roles.map(role=>role.rights),...definition.steps.map(step=>step.rights),...calls.map(call=>call.rights)]
    .some(rights=>rights.tools.some(tool=>["document_read","document_write","image_edit"].includes(tool)))
    ||calls.some(call=>workflowRequiresDocumentEngine(call.definition,depth+1));
}

/** A retained original intent wins over a newly inspected source or target selection. */
export function WorkflowSnapshotExecutionReview({client,request,onStarted,onRetired}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [original]=useState(()=>new PersistentRequestLatch<PrepareOwnerExecutionPlanV2>(workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",request.order_id,request.idempotency_key])),value=>decode("PrepareOwnerExecutionPlanV2",value)));
  const [preferredTarget]=useState(()=>defaultExecutionTarget(client));
  const [needsDocuments,setNeedsDocuments]=useState<boolean|null>(null),[selected,setSelected]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(""),[attempt,setAttempt]=useState(0);
  useEffect(()=>{let active=true;
    if(original.pending||!("kind" in request))return;
    setError("");
    const workflow=request;
    async function load(){
      const order=await client.order(workflow.order_id);
      const version=await client.workflowVersion(order.project_id,workflow.workflow_id,workflow.version_id);
      if(version.reference.id!==workflow.version_id||version.project_id!==order.project_id||version.workflow_id!==workflow.workflow_id)throw new Error("execution_plan_original_mismatch");
      const required=workflowRequiresDocumentEngine(version.definition);
      if(active)setNeedsDocuments(required);
    }
    void load().catch(reason=>{if(active)setError(reason instanceof Error?reason.message:String(reason));});
    return()=>{active=false;};
  },[client,request,original,attempt]);
  if(original.pending||!("kind" in request)||needsDocuments===false||selected||needsDocuments&&preferredTarget?.kind==="document_engine")return <SnapshotExecutionPlanReview client={client} request={request} executionTarget={original.pending?undefined:selected?documentTarget:preferredTarget??undefined} onStarted={onStarted} onRetired={onRetired}/>;
  async function retire(){if(busy)return;setBusy(true);setError("");
    try{await client.retireExecutionRequest(request);onRetired();}
    catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  return <section className="product-item execution-plan-review" aria-label={tr("Dokumentausführung auswählen","Choose document execution")}>
    <h3>{tr("Dokumentausführung auswählen","Choose document execution")}</h3>
    {error&&<p role="alert" className="error">{workflowDataError(error,language)}</p>}
    {needsDocuments===null&&!error&&<p role="status">{tr("Die veröffentlichte Workflowversion wird geprüft…","Checking the published workflow version…")}</p>}
    {needsDocuments&&<>
      <p>{tr("Dieser Ablauf benötigt die lokale Dokumentengine. Sie verarbeitet die gewählten Dateikopien mit Dokumentprogrammen unter deinen Betriebssystem- und Netzwerkrechten. Die Ordnerauswahl ist keine Betriebssystemisolation.","This workflow needs the local document engine. It processes the selected file copies using document programs under your operating system and network permissions. Folder selection is not operating system isolation.")}</p>
      <p>{tr("Anschließend prüfst du den genauen Plan, die Modellanbieter und Freigaben vor dem Start.","You will then review the exact plan, model providers and approvals before starting.")}</p>
      <button type="button" className="primary" disabled={busy} onClick={()=>setSelected(true)}>{tr("Dokumentengine verwenden","Use document engine")}</button>
    </>}
    {error&&<button type="button" disabled={busy} onClick={()=>setAttempt(value=>value+1)}>{tr("Workflowversion erneut prüfen","Recheck workflow version")}</button>}
    <button type="button" disabled={busy} onClick={()=>void retire()}>{tr("Anfrage verbindlich verwerfen","Retire original request")}</button>
  </section>;
}
