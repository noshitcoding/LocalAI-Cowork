import {useEffect,useState,type ReactNode} from "react";
import {Client,decode,type CreateRun,type PrepareOwnerExecutionPlanV2,type RunRecord} from "@cowork/client";
import {environmentRecords,type EnvironmentRecord} from "./EnvironmentPanel";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {SnapshotExecutionPlanReview} from "./SnapshotExecutionPlanReview";
import {workflowDataError} from "./workflowDataflow";
import {useView} from "./view";

const registeredTarget:PrepareOwnerExecutionPlanV2["target"]={kind:"registered_runner"};
type Selection={kind:"managed"|"existing";name:string};
type Props={client:Client;request:CreateRun;fallback:ReactNode;onStarted:(run:RunRecord)=>Promise<void>;onRetired:()=>void};

/** Classify only the exact selected active version. This never grants execution. */
export function environmentExecutionSelection(request:CreateRun,project:string,records:EnvironmentRecord[]):Selection{
  const ref=request.environment;
  const candidates=records.filter(record=>record.id===ref?.environment_id);
  if(!ref||candidates.length!==1)throw new Error("environment_execution_selection_changed");
  const record=candidates[0],active=record.active_reference,versions=record.versions.filter(version=>version.id===ref.version_id);
  if(record.project_id!==project||record.revoked||record.active_version_id!==ref.version_id||versions.length!==1
    ||!active||(["environment_id","version_id","definition_sha256","qualification_sha256"] as const).some(key=>active[key]!==ref[key]))throw new Error("environment_execution_selection_changed");
  const version=versions[0];
  if(version.revoked||version.state.state!=="qualified"||version.sha256!==ref.definition_sha256
    ||version.state.qualification.definition_sha256!==ref.definition_sha256)throw new Error("environment_execution_selection_changed");
  if(version.definition.artifact.kind==="managed_linux_package"){
    if(version.definition.format_version!==3||version.definition.network?.kind!=="denied"
      ||request.native_tools?.length||request.mcp_tools?.length)throw new Error("environment_execution_selection_unsupported");
    return {kind:"managed",name:record.name};
  }
  if(!["linux_image","linux_kata_image","local_linux_image","windows_bundle"].includes(version.definition.artifact.kind))throw new Error("environment_execution_selection_unsupported");
  return {kind:"existing",name:record.name};
}

export function ManagedEnvironmentConsent({name,busy,choose,retire}:{name:string;busy:boolean;choose:()=>void;retire:()=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  return <>
    <p>{tr("Die gewählte Umgebung","The selected environment")} <strong>{name}</strong> {tr("verwendet die registrierte Runtime. Sie erhält eine eigene Kopie der ausgewählten Dateien; das Prozessnetzwerk bleibt gesperrt.","uses the registered runtime. It receives its own copy of the selected files; process networking remains denied.")}</p>
    <p>{tr("Anschließend prüfst und bestätigst du den genauen Plan, die Modellanbieter und Rechte. Ziel und Dienstbereitschaft werden vor der Vorbereitung geprüft.","Next, review and confirm the exact plan, model providers and permissions. The target and service readiness are checked before preparation.")}</p>
    <button type="button" className="primary" disabled={busy} onClick={choose}>{tr("Arbeitskopie mit registrierter Runtime prüfen","Review working copy with registered runtime")}</button>
    <button type="button" disabled={busy} onClick={retire}>{tr("Anfrage verbindlich verwerfen","Retire original request")}</button>
  </>;
}

/** Original intent wins after reload; failed resolution never selects another backend. */
export function EnvironmentSnapshotExecutionReview({client,request,fallback,onStarted,onRetired}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [original]=useState(()=>new PersistentRequestLatch<PrepareOwnerExecutionPlanV2>(workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",request.order_id,request.idempotency_key])),value=>decode("PrepareOwnerExecutionPlanV2",value)));
  const [selection,setSelection]=useState<Selection|null>(null),[selected,setSelected]=useState(false),[error,setError]=useState(""),[busy,setBusy]=useState(false),[attempt,setAttempt]=useState(0);
  useEffect(()=>{let active=true;
    if(original.pending||original.invalid)return;
    setSelection(null);
    setError("");
    async function load(){
      const order=await client.order(request.order_id);
      const records=await environmentRecords(client,order.project_id);
      const value=environmentExecutionSelection(request,order.project_id,records);
      if(active)setSelection(value);
    }
    void load().catch(reason=>{if(active)setError(reason instanceof Error?reason.message:String(reason));});
    return()=>{active=false;};
  },[client,request,original,attempt]);
  if(original.pending||selected)return <SnapshotExecutionPlanReview client={client} request={request} executionTarget={selected?registeredTarget:undefined} onStarted={onStarted} onRetired={onRetired}/>;
  if(selection?.kind==="existing"&&!error&&!original.invalid)return fallback;
  async function retire(){if(busy||original.invalid)return;setBusy(true);setError("");
    try{await client.retireExecutionRequest(request);onRetired();}
    catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  const problem=original.invalid?tr("Die gespeicherte ursprüngliche Anfrage kann nicht gelesen werden.","The saved original request cannot be read.")
    :error==="environment_execution_selection_changed"?tr("Die ausgewählte Umgebung oder ihre aktive Version wurde geändert. Verwirf diese Anfrage und wähle die Umgebung erneut.","The selected environment or its active version changed. Retire this request and select the environment again.")
    :error==="environment_execution_selection_unsupported"?tr("Diese Umgebung unterstützt die gewählte Kombination von Programm- und Verbindungsrechten nicht.","This environment does not support the selected combination of program and connector permissions.")
    :error?workflowDataError(error,language):"";
  return <section className="product-item execution-plan-review" aria-label={tr("Runtime auswählen","Choose runtime")}>
    <h3>{tr("Runtime auswählen","Choose runtime")}</h3>
    {problem&&<p role="alert" className="error">{problem}</p>}
    {!selection&&!problem&&<p role="status">{tr("Die ausgewählte Umgebung wird geprüft…","Checking the selected environment…")}</p>}
    {selection?.kind==="managed"&&!problem?<ManagedEnvironmentConsent name={selection.name} busy={busy} choose={()=>setSelected(true)} retire={()=>void retire()}/>:<>
      {!!error&&<button type="button" disabled={busy||original.invalid} onClick={()=>setAttempt(value=>value+1)}>{tr("Umgebung erneut prüfen","Recheck environment")}</button>}
      <button type="button" disabled={busy||original.invalid} onClick={()=>void retire()}>{tr("Anfrage verbindlich verwerfen","Retire original request")}</button>
    </>}
  </section>;
}
