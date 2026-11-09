import {useState} from "react";
import {Client,decode,type CreateRun,type PrepareOwnerExecutionPlanV2,type RunRecord} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {SnapshotExecutionPlanReview} from "./SnapshotExecutionPlanReview";
import {workflowDataError} from "./workflowDataflow";
import {useView} from "./view";
import {defaultExecutionTarget} from "./executionDefaults";

type Target=PrepareOwnerExecutionPlanV2["target"];
const brokerTarget:Target={kind:"broker_controller"},documentTarget:Target={kind:"document_engine"};
type Props={client:Client;request:CreateRun;onStarted:(run:RunRecord)=>Promise<void>;onRetired:()=>void};

export function SingleExecutionChoice({busy,choose,retire}:{busy:boolean;choose:(target:Target)=>void;retire:()=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  return <>
    <p>{tr("Wähle, wie dieser Auftrag die Arbeitskopie verarbeiten darf. Anschließend prüfst und bestätigst du den genauen Plan, die Modellanbieter und Rechte.","Choose how this task may process its working copy. You will then review and confirm the exact plan, model providers and permissions.")}</p>
    <div className="product-item">
      <h4>{tr("Dateien und Text","Files and text")}</h4>
      <p>{tr("Begrenzte Datei- und Artefaktwerkzeuge ohne allgemeine Programme.","Bounded file and artifact tools without general programs.")}</p>
      <button type="button" className="primary" disabled={busy} onClick={()=>choose(brokerTarget)}>{tr("Mit Dateibroker prüfen","Review with file broker")}</button>
    </div>
    <div className="product-item">
      <h4>{tr("Dokumente und Bilder","Documents and images")}</h4>
      <p>{tr("Die lokale Dokumentengine verarbeitet die gewählten Dateikopien mit Dokumentprogrammen unter deinen Betriebssystem- und Netzwerkrechten. Die Ordnerauswahl ist keine Betriebssystemisolation.","The local document engine processes the selected file copies using document programs under your operating system and network permissions. Folder selection is not operating system isolation.")}</p>
      <button type="button" disabled={busy} onClick={()=>choose(documentTarget)}>{tr("Dokumentengine verwenden","Use document engine")}</button>
    </div>
    <button type="button" disabled={busy} onClick={retire}>{tr("Anfrage verbindlich verwerfen","Retire original request")}</button>
  </>;
}

/** Freeze the configured default once per request. A saved original always wins on reload. */
export function SingleSnapshotExecutionReview({client,request,onStarted,onRetired}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [original]=useState(()=>new PersistentRequestLatch<PrepareOwnerExecutionPlanV2>(workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",request.order_id,request.idempotency_key])),value=>decode("PrepareOwnerExecutionPlanV2",value)));
  const [selected,setSelected]=useState<Target|null>(()=>defaultExecutionTarget(client)),[busy,setBusy]=useState(false),[error,setError]=useState("");
  if(original.pending||selected)return <SnapshotExecutionPlanReview client={client} request={request} executionTarget={original.pending?undefined:selected??undefined} onStarted={onStarted} onRetired={onRetired}/>;
  async function retire(){if(busy)return;setBusy(true);setError("");
    try{await client.retireExecutionRequest(request);onRetired();}
    catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  return <section className="product-item execution-plan-review" aria-label={tr("Ausführungsart auswählen","Choose execution mode")}>
    <h3>{tr("Ausführungsart auswählen","Choose execution mode")}</h3>
    {error&&<p role="alert" className="error">{workflowDataError(error,language)}</p>}
    <SingleExecutionChoice busy={busy} choose={setSelected} retire={()=>void retire()}/>
  </section>;
}
