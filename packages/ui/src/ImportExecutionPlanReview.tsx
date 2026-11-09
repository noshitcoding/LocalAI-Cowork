import {useEffect,useRef,useState} from "react";
import {Client,decode,type CreateImportRun,type PrepareExecutionInput,type PrepareImportExecutionPlanV2,type PreparedExecutionPlanV2,type RunRecord} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {useView} from "./view";
import {workflowDataError} from "./workflowDataflow";

type Confirmation={order_id:string;plan_id:string;sha256:string};
function confirmation(value:unknown):Confirmation{
  const v=value as Confirmation;
  if(!v||typeof v!=="object"||Object.keys(v).sort().join(",")!=="order_id,plan_id,sha256"||typeof v.order_id!=="string"||!v.order_id||typeof v.plan_id!=="string"||!v.plan_id||typeof v.sha256!=="string"||!/^[a-f0-9]{64}$/.test(v.sha256))throw new Error("request_journal_invalid");
  return v;
}

export function ImportExecutionPlanReview({client,request,onStarted,onRetired}:{client:Client;request:CreateImportRun;onStarted:(run:RunRecord)=>Promise<void>;onRetired:()=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const key=(stage:string)=>workspaceJournalKey(client,JSON.stringify(["import-plan-v2",request.order_id,request.idempotency_key,stage]));
  const [selection]=useState(()=>new PersistentRequestLatch<PrepareExecutionInput>(key("selection"),v=>decode("PrepareExecutionInput",v)));
  const [intent]=useState(()=>new PersistentRequestLatch<PrepareImportExecutionPlanV2>(key("intent"),v=>decode("PrepareImportExecutionPlanV2",v)));
  const [committing]=useState(()=>new PersistentRequestLatch<Confirmation>(key("confirmation"),confirmation));
  const [prepared,setPrepared]=useState<PreparedExecutionPlanV2|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(""),[attempt,setAttempt]=useState(0),[pending,setPending]=useState(committing.pending);
  const stopped=useRef(false),invalid=selection.invalid||intent.invalid||committing.invalid;
  useEffect(()=>{let active=true;stopped.current=false;setError("");
    async function load(){
      if(invalid)throw new Error("request_journal_invalid");
      let original:PrepareImportExecutionPlanV2;
      if(intent.pending)original=intent.prepare(()=>{throw new Error("execution_plan_original_missing");});
      else{
        const selected=selection.prepare(()=>({idempotency_key:request.idempotency_key,base_revision:request.base_revision,folder_ids:[request.import.folder_id]}));
        const input=await client.prepareExecutionInput(request.order_id,selected);
        if(!active||stopped.current)return;
        if(input.retired)throw new Error("execution_input_retired");
        original=intent.prepare(()=>({target:{kind:"broker_controller"},request:structuredClone(request),input:{snapshot_id:input.snapshot.id,sha256:input.sha256,manifest_sha256:input.snapshot.manifest.sha256}}));
      }
      if(JSON.stringify(original.request)!==JSON.stringify(request))throw new Error("execution_plan_original_mismatch");
      const plan=await client.prepareImportExecutionPlanV2(original);
      if(committing.pending){const saved=committing.prepare(()=>{throw new Error("execution_plan_original_missing");});if(saved.order_id!==plan.plan.order_id||saved.plan_id!==plan.plan.id||saved.sha256!==plan.sha256)throw new Error("execution_plan_original_mismatch");}
      if(active&&!stopped.current)setPrepared(plan);
    }
    void load().catch(reason=>{if(active&&!stopped.current)setError(reason instanceof Error?reason.message:String(reason));});
    return()=>{active=false;};
  },[client,request,selection,intent,committing,invalid,attempt]);
  function clear(){committing.confirm();intent.confirm();selection.confirm();}
  async function commit(){if(busy||!prepared||invalid)return;setBusy(true);setError("");
    try{const saved=committing.prepare(()=>({order_id:prepared.plan.order_id,plan_id:prepared.plan.id,sha256:prepared.sha256}));setPending(true);
      if(saved.order_id!==prepared.plan.order_id||saved.plan_id!==prepared.plan.id||saved.sha256!==prepared.sha256)throw new Error("execution_plan_original_mismatch");
      const run=await client.commitImportExecutionPlanV2(prepared);await onStarted(run);clear();setPending(false);
    }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  async function retire(){if(busy||invalid)return;setBusy(true);setError("");stopped.current=true;
    try{
      if(prepared)await client.retireExecutionPlanV2(prepared);
      else if(intent.pending)await client.retireImportExecutionRequestV2(intent.prepare(()=>{throw new Error("execution_plan_original_missing");}));
      else await client.retireImportExecutionIntentV2(request);
      clear();onRetired();
    }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  return <section className="product-item execution-plan-review" aria-label={tr("Import prüfen","Review import")}>
    <h3>{tr("Import prüfen","Review import")}</h3>
    {error&&<p role="alert" className="error">{workflowDataError(error,language)}</p>}
    {!prepared&&!error&&<p role="status">{tr("Upload und neue Arbeitskopie werden geprüft…","Checking the upload and new working copy…")}</p>}
    {prepared&&<ImportPlanSummary prepared={prepared} target={client.base||globalThis.location?.origin||""}/>}
    {pending&&<p role="status">{tr("Die ursprüngliche Bestätigung ist gespeichert. Gleiche denselben Start ab.","The original confirmation is saved. Reconcile the same start.")}</p>}
    <div className="actions">
      {prepared&&<button type="button" className="primary" disabled={busy||invalid||prepared.retired} onClick={()=>void commit()}>{pending?tr("Ursprünglichen Start abgleichen","Reconcile original start"):prepared.committed_run_id?tr("Bestätigten Start anzeigen","Show accepted start"):tr("Plan bestätigen und Import starten","Confirm plan and start import")}</button>}
      {error&&!prepared&&<button type="button" disabled={busy||invalid} onClick={()=>setAttempt(n=>n+1)}>{tr("Ursprüngliche Anfrage erneut prüfen","Recheck original request")}</button>}
      <button type="button" disabled={busy||invalid} onClick={()=>void retire()}>{tr("Auftrag verbindlich verwerfen","Retire this task")}</button>
    </div>
  </section>;
}

export function ImportPlanSummary({prepared,target}:{prepared:PreparedExecutionPlanV2;target:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const p=prepared.plan;if(p.preflight.kind!=="import")return null;const review=p.preflight.review,action=review.import;
  const folder=review.folders.find(folder=>folder.id===action.folder_id);
  return <>
    <p>{tr("Der Upload wird in einer neuen Arbeitskopie importiert. Originaldateien, frühere Ergebnisse und dein Chatentwurf bleiben erhalten.","The upload is imported into a new working copy. Original files, earlier results and your chat draft are preserved.")}</p>
    <dl>
      <dt>{tr("Ausführungsziel","Destination")}</dt><dd>{target} · {tr("Dateibroker","File broker")}</dd>
      <dt>{tr("Ausgewählte Eingaben","Selected inputs")}</dt><dd>{p.input.manifest.files.length} {tr("Dateien","files")} · {p.input.manifest.total_bytes} B</dd>
      <dt>{tr("Hochgeladene Datei","Uploaded file")}</dt><dd>{review.upload.file.file_name} · {review.upload.file.size} B</dd>
      <dt>{tr("Ziel in der neuen Kopie","Destination in the new copy")}</dt><dd>{folder?.name} / {action.path}</dd>
      <dt>{tr("Dateiaktion","File action")}</dt><dd>{action.expected_sha256?tr("Beobachtete Dateiversion in dieser Kopie ersetzen","Replace the observed version in this copy"):tr("Neue Datei anlegen","Create a new file")}</dd>
      <dt>{tr("Modellanfragen","Model requests")}</dt><dd>{tr("Keine – Import ohne Modell","None — import without a model")}</dd>
      <dt>{tr("Freigabe","Approval")}</dt><dd>{review.access_mode==="autonomous"?tr("Innerhalb der Freigabe autonom","Autonomous within the grant"):tr("Genaue Dateiaktion bestätigen","Confirm the exact file action")}</dd>
      <dt>{tr("Grenzen","Limits")}</dt><dd>{review.budgets.max_seconds} s · {review.budgets.max_steps} {tr("Schritte höchstens","maximum steps")}</dd>
    </dl>
    <p>{tr("Nach Abschluss kannst du das Ergebnis herunterladen oder mit „Ab hier verzweigen“ als neue Arbeitsgrundlage verwenden.","After completion, download the result or use ‘Branch from here’ to start from its files.")}</p>
    <details><summary>{tr("SHA-256 und Herkunft","SHA-256 and provenance")}</summary><p>{tr("Upload","Upload")}: <code>{action.file.sha256}</code></p>{action.expected_sha256&&<p>{tr("Beobachtete Zielversion","Observed destination version")}: <code>{action.expected_sha256}</code></p>}<p>{tr("Plan","Plan")}: <code>{prepared.sha256}</code></p></details>
  </>;
}
