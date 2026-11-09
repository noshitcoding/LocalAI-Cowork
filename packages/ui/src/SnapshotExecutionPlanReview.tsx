import {useEffect,useState} from "react";
import {Client,decode,defaultRunBudgets,canonicalJsonValue,type PreparedExecutionPlanV2,type PrepareExecutionInput,type PrepareOwnerExecutionPlanV2,type RunRecord,type RunRequest} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {useView} from "./view";
import {WorkflowDataValuePreview} from "./WorkflowDataflowEditor";
import {workflowDataError} from "./workflowDataflow";

type Confirmation={order_id:string;plan_id:string;sha256:string};
function confirmation(value:unknown):Confirmation {
  const v=value as Confirmation;
  if(!v||typeof v!=="object"||Object.keys(v).sort().join(",")!=="order_id,plan_id,sha256"||typeof v.order_id!=="string"||!v.order_id||typeof v.plan_id!=="string"||!v.plan_id||typeof v.sha256!=="string"||!/^[a-f0-9]{64}$/.test(v.sha256))throw new Error("request_journal_invalid");
  return v;
}
type Props={client:Client;request:RunRequest;executionTarget?:PrepareOwnerExecutionPlanV2["target"];onStarted:(run:RunRecord)=>Promise<void>;onRetired:()=>void};

/** Keep the small original intent/confirmation, not a multi-megabyte manifest. */
export function SnapshotExecutionPlanReview({client,request,executionTarget,onStarted,onRetired}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [selection]=useState(()=>new PersistentRequestLatch<PrepareExecutionInput>(workspaceJournalKey(client,JSON.stringify(["snapshot-input-selection",request.order_id,request.idempotency_key])),v=>decode("PrepareExecutionInput",v)));
  const [intent]=useState(()=>new PersistentRequestLatch<PrepareOwnerExecutionPlanV2>(workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",request.order_id,request.idempotency_key])),v=>decode("PrepareOwnerExecutionPlanV2",v)));
  const [committing]=useState(()=>new PersistentRequestLatch<Confirmation>(workspaceJournalKey(client,JSON.stringify(["snapshot-execution-confirmation",request.order_id,request.idempotency_key])),confirmation));
  const [prepared,setPrepared]=useState<PreparedExecutionPlanV2|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(""),[attempt,setAttempt]=useState(0),[pending,setPending]=useState(committing.pending);
  const invalid=selection.invalid||intent.invalid||committing.invalid;
  useEffect(()=>{let active=true;setError("");
    async function load(){
      if(invalid)throw new Error("request_journal_invalid");
      let original:PrepareOwnerExecutionPlanV2;
      if(intent.pending)original=intent.prepare(()=>{throw new Error("execution_plan_original_missing");});
      else{
        let chosen:PrepareExecutionInput;
        if(selection.pending)chosen=selection.prepare(()=>{throw new Error("execution_plan_original_missing");});
        else{
          const folders=(request.folders??[]).map(f=>f.id);
          if(folders.length===0)folders.push((await client.workspace(request.order_id)).folder.id);
          if(!active)return;
          chosen=selection.prepare(()=>({idempotency_key:request.idempotency_key,base_revision:request.base_revision,folder_ids:[...new Set(folders)].sort()}));
        }
        if(!active)return;
        const input=await client.prepareExecutionInput(request.order_id,chosen);
        if(input.retired)throw new Error("execution_input_retired");
        if(!active)return;
        original=intent.prepare(()=>({target:executionTarget??{kind:"broker_controller"},request:structuredClone(request),input:{snapshot_id:input.snapshot.id,sha256:input.sha256,manifest_sha256:input.snapshot.manifest.sha256}}));
      }
      if(original.request.order_id!==request.order_id||original.request.idempotency_key!==request.idempotency_key)throw new Error("execution_plan_original_mismatch");
      const plan=await client.prepareExecutionPlanV2(original);
      if(committing.pending){const receipt=committing.prepare(()=>{throw new Error("execution_plan_original_missing");});
        if(receipt.order_id!==plan.plan.order_id||receipt.plan_id!==plan.plan.id||receipt.sha256!==plan.sha256)throw new Error("execution_plan_original_mismatch");}
      if(active)setPrepared(plan);
    }
    void load().catch(reason=>{if(active)setError(reason instanceof Error?reason.message:String(reason));});
    return()=>{active=false;};
  },[client,request,executionTarget,selection,intent,committing,invalid,attempt]);
  function clear(){committing.confirm();intent.confirm();selection.confirm();}
  async function commit(){if(busy||!prepared||invalid)return;setBusy(true);setError("");
    try{const receipt=committing.prepare(()=>({order_id:prepared.plan.order_id,plan_id:prepared.plan.id,sha256:prepared.sha256}));setPending(true);
      if(receipt.plan_id!==prepared.plan.id||receipt.sha256!==prepared.sha256)throw new Error("execution_plan_original_mismatch");
      const run=await client.commitExecutionPlanV2(prepared);clear();setPending(false);await onStarted(run);
    }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  async function retire(){if(busy||invalid)return;setBusy(true);setError("");
    try{
      if(prepared)await client.retireExecutionPlanV2(prepared);
      else if(intent.pending)await client.retireExecutionRequestV2(intent.prepare(()=>{throw new Error("execution_plan_original_missing");}));
      else await client.retireExecutionRequest(request); // Authenticated cross-version key fence before any PlanV2 exists.
      clear();onRetired();
    }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  return <section className="product-item execution-plan-review" aria-label={tr("Ausführung prüfen","Review execution")}>
    <h3>{tr("Ausführung prüfen","Review execution")}</h3>
    {error&&<p role="alert" className="error">{workflowDataError(error,language)}</p>}
    {!prepared&&!error&&<p role="status">{tr("Ausgewählte Dateien werden eingefroren und die Auftragsrechte geprüft…","Freezing selected files and checking task permissions…")}</p>}
    {prepared&&<SnapshotPlanSummary prepared={prepared} target={client.base||globalThis.location?.origin||""}/>}
    {pending&&<p role="status">{tr("Die ursprüngliche Bestätigung ist gespeichert. Gleiche denselben Start ab.","The original confirmation is saved. Reconcile the same start.")}</p>}
    <div className="actions">
      {prepared&&<button type="button" className="primary" disabled={busy||invalid||prepared.retired} onClick={()=>void commit()}>{pending?tr("Ursprünglichen Start abgleichen","Reconcile original start"):prepared.committed_run_id?tr("Bestätigten Start anzeigen","Show accepted start"):tr("Plan bestätigen und Arbeitskopie starten","Confirm plan and start working copy")}</button>}
      {error&&!prepared&&<button type="button" disabled={busy||invalid} onClick={()=>setAttempt(n=>n+1)}>{tr("Ursprüngliche Anfrage erneut prüfen","Recheck original request")}</button>}
      <button type="button" disabled={busy||invalid} onClick={()=>void retire()}>{tr("Auftrag verbindlich verwerfen","Retire this task")}</button>
    </div>
  </section>;
}

export function SnapshotPlanSummary({prepared,target}:{prepared:PreparedExecutionPlanV2;target:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,p=prepared.plan;
  if(p.preflight.kind!=="owner")return null;
  const flight=p.preflight.review,models="provider" in flight?[flight.provider]:flight.roles.map(r=>r.provider);
  const runner=p.target.kind==="managed_runner"?p.target:null,documents=p.target.kind==="document_engine";
  return <>
    <p>{tr("Dieser Auftrag arbeitet mit einer eigenen Dateikopie. Sein Ergebnis wird als neue Inhaltsversion gespeichert.","This task works with its own file copy. Its result is saved as a new content version.")}</p>
    <dl><dt>{tr("Ausführungsziel","Execution target")}</dt><dd>{target} · {runner?runner.runner.runner_id:documents?tr("Dokumentengine","Document engine"):tr("Dateibroker","File broker")}</dd>
      {"workflow" in flight&&<><dt>{tr("Workflow","Workflow")}</dt><dd>{flight.name} · {tr("Version","Version")} {flight.version_revision}</dd></>}
      <dt>{tr("Eingefrorene Eingaben","Frozen inputs")}</dt><dd>{p.input.sources.length?p.input.sources.map(s=><div key={s.alias}>{s.name}</div>):tr("Leere Arbeitskopie","Empty working copy")}<small>{p.input.manifest.files.length} {tr("Dateien","files")} · {p.input.manifest.total_bytes.toLocaleString(language)} B</small></dd>
      <dt>{tr("Rechte in der Arbeitskopie","Working copy permissions")}</dt><dd>{p.boundary.folders.length?p.boundary.folders.map(f=><div key={f.id}>{f.name} · {f.writable?tr("Lesen und Schreiben","Read and write"):tr("Nur Lesen","Read only")}</div>):tr("Keine Dateiwerkzeuge","No file tools")}</dd>
      <dt>{tr("Modellanfragen","Model requests")}</dt><dd>{models.length===0?tr("Keine","None"):models.map((model,index)=><div key={model.id+":"+index}>{model.name} · {model.model} · {model.kind==="test"?tr("Testanbieter","Test provider"):model.endpoint||model.kind}</div>)}</dd>
      <dt>{tr("Programmzugriff","Program access")}</dt><dd>{runner?<>{tr("Programme der gewählten Umgebung","Programs in the selected environment")}<ul>{Object.keys(runner.programs).map(name=><li key={name}>{name}</li>)}</ul></>:documents?tr("Dokumentprogramme mit deinen Betriebssystemrechten. Die Ordnerauswahl ist keine Betriebssystemisolation.","Document programs use your operating system permissions. Folder selection is not operating system isolation."):tr("Begrenzte Datei- und Artefaktwerkzeuge","Bounded file and artifact tools")}</dd>
      {documents&&<><dt>{tr("Prozessnetzwerk","Process network")}</dt><dd>{tr("Netzwerkrechte deines Benutzerkontos","Network permissions of your user account")}</dd></>}
      {runner&&<><dt>{tr("Prozessnetzwerk","Process network")}</dt><dd>{tr("Gesperrt","Denied")}</dd><dt>{tr("Einzelner Programmlauf","Individual program execution")}</dt><dd>{tr("Höchstens 30 Sekunden mit der anfänglichen Freigabe; eine kürzere Ablaufzeit bleibt verbindlich.","At most 30 seconds under the initial authorization; an earlier expiration remains binding.")}</dd></>}
      <dt>{tr("Freigabemodus","Approval mode")}</dt><dd>{flight.access_mode==="autonomous"?tr("Vorab erlaubte Aktionen automatisch","Preauthorized actions run automatically"):flight.access_mode==="read_only"?tr("Nur Lesen","Read only"):flight.access_mode==="confirm_all"?tr("Jede Werkzeugaktion bestätigen","Confirm every tool action"):tr("Verändernde Aktionen bestätigen","Confirm changes")}</dd>
      <dt>{tr("Grenzen","Limits")}</dt><dd>{flight.budgets.max_seconds} s · {flight.budgets.max_steps} {tr("Schritte","steps")} · {flight.budgets.max_total_tokens??defaultRunBudgets.max_total_tokens} {tr("Tokens höchstens","maximum tokens")}</dd>
    </dl>
    <p className="muted">{tr("Inhalte, die der Agent liest, können an die angezeigten Modellanbieter übertragen werden. Änderungen an Originaldateien benötigen eine gesonderte Übernahme.","Content the agent reads may be sent to the model providers shown. Changes to original files require a separate adoption.")}</p>
    {"workflow" in flight&&flight.workflow.inputs!==undefined&&<section className="product-item"><h4>{tr("Gebundene Workflow-Eingaben","Bound workflow inputs")}</h4>{flight.input_source?<pre>{canonicalJsonValue(prepared.canonical_json,["preflight","review","workflow","inputs"])}</pre>:<WorkflowDataValuePreview value={flight.workflow.inputs}/>}</section>}
    <details><summary>{tr("Werkzeuge und Planbeleg anzeigen","Show tools and plan receipt")}</summary><ul>{flight.allowed_tools.map(tool=><li key={tool}>{tool}</li>)}</ul><p>{tr("Plan-Fingerprint: ","Plan fingerprint: ")}<code>{prepared.sha256}</code></p></details>
  </>;
}
