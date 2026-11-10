import {useEffect,useState} from "react";
import {Client,decode,defaultRunBudgets,type PreparedExecutionPlan,type RunRecord,type RunRequest} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {useView} from "./view";
import {WorkflowDataValuePreview} from "./WorkflowDataflowEditor";
import {workflowDataError} from "./workflowDataflow";
import {SingleSnapshotExecutionReview} from "./SingleSnapshotExecutionReview";
import {WorkflowSnapshotExecutionReview} from "./WorkflowSnapshotExecutionReview";
import {EnvironmentSnapshotExecutionReview} from "./EnvironmentSnapshotExecutionReview";

type Props={client:Client,request:RunRequest,onStarted:(run:RunRecord)=>Promise<void>,onRetired:()=>void};

/** Stored confirmation is an immutable original intent, never a reusable grant. */
export function ExecutionPlanReview({client,request,onStarted,onRetired}:Props){
  const [legacy]=useState(()=>new PersistentRequestLatch<PreparedExecutionPlan>(
    workspaceJournalKey(client,JSON.stringify(["execution-plan-confirmation",request.order_id,request.idempotency_key])),
    value=>decode("PreparedExecutionPlan",value),
  ).pending);
  if(!legacy&&!request.environment&&!request.native_tools?.length&&!request.mcp_tools?.length)return "kind" in request?<WorkflowSnapshotExecutionReview key={request.idempotency_key} client={client} request={request} onStarted={onStarted} onRetired={onRetired}/>:<SingleSnapshotExecutionReview key={request.idempotency_key} client={client} request={request} onStarted={onStarted} onRetired={onRetired}/>;
  if(!legacy&&request.environment&&!("kind" in request))return <EnvironmentSnapshotExecutionReview key={request.idempotency_key} client={client} request={request} onStarted={onStarted} onRetired={onRetired} fallback={<LegacyExecutionPlanReview client={client} request={request} onStarted={onStarted} onRetired={onRetired}/>}/>;
  return <LegacyExecutionPlanReview client={client} request={request} onStarted={onStarted} onRetired={onRetired}/>;
}
function LegacyExecutionPlanReview({client,request,onStarted,onRetired}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [confirmation]=useState(()=>new PersistentRequestLatch<PreparedExecutionPlan>(
    workspaceJournalKey(client,JSON.stringify(["execution-plan-confirmation",request.order_id,request.idempotency_key])),
    value=>decode("PreparedExecutionPlan",value),
  ));
  const [prepared,setPrepared]=useState<PreparedExecutionPlan|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const [pending,setPending]=useState(confirmation.pending);
  const [attempt,setAttempt]=useState(0);
  useEffect(()=>{let active=true;setError("");
    const load=async()=>{
      if(confirmation.invalid)throw new Error("request_journal_invalid");
      const plan=confirmation.pending?confirmation.prepare(()=>{throw new Error("execution_plan_original_missing");}):await client.prepareExecutionPlan(request);
      if(plan.plan.order_id!==request.order_id||plan.plan.idempotency_key!==request.idempotency_key)throw new Error("execution_plan_original_mismatch");
      if(active)setPrepared(plan);
    };
    void load().catch(reason=>{if(active)setError(reason instanceof Error?reason.message:String(reason));});
    return()=>{active=false;};
  },[client,request,confirmation,attempt]);
  async function commit(){if(busy||!prepared)return;setBusy(true);setError("");
    try{const original=confirmation.prepare(()=>prepared);setPending(true);const run=await client.commitExecutionPlan(original);
      confirmation.confirm();setPending(false);await onStarted(run);
    }catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  async function retire(){if(busy)return;setBusy(true);setError("");
    try{if(prepared)await client.retireExecutionPlan(prepared);else await client.retireExecutionRequest(request);confirmation.discard();onRetired();}
    catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);}
  }
  const flight=prepared?.plan.preflight,boundary=prepared?.plan.boundary;
  const models=flight?("provider" in flight?[flight.provider]:flight.roles.map(role=>role.provider)):[];
  const budget=flight?.budgets;
  const network=boundary?.process_network;
  const access=boundary?.process_access;
  const target=client.base||globalThis.location?.origin||"";
  return <section className="product-item execution-plan-review" aria-label={tr("Ausführung prüfen","Review execution")}>
    <h3>{tr("Ausführung prüfen","Review execution")}</h3>
    {error&&<p role="alert" className="error">{workflowDataError(error,language)}</p>}
    {error&&!prepared&&<button type="button" disabled={confirmation.invalid} onClick={()=>setAttempt(value=>value+1)}>{tr("Ursprüngliche Anfrage erneut prüfen","Recheck original request")}</button>}
    {error&&!prepared&&<button type="button" disabled={busy||confirmation.invalid} onClick={()=>void retire()}>{tr("Anfrage verbindlich verwerfen","Retire original request")}</button>}
    {!prepared&&!error&&<p role="status">{tr("Berechtigungen und Ausführungsziel werden geprüft…","Checking permissions and execution target…")}</p>}
    {prepared&&<><dl>
      <dt>{tr("Ausführungsziel","Execution target")}</dt><dd>{target}</dd>
      <dt>{tr("Programmrechte","Program access")}</dt><dd>{access==="user_rights"?tr("Mit deinen Betriebssystemrechten; Ordnerauswahl begrenzt Programme nicht.","Your operating system permissions; folder selection does not confine programs."):access==="qualified_environment"?tr("Explizit gewählte qualifizierte Umgebung","Explicitly selected qualified environment"):tr("Dateibroker; keine allgemeinen Programme","File broker; no general programs")}</dd>
      <dt>{tr("Modellanfragen","Model requests")}</dt><dd>{models.length===0&&tr("Keine","None")}{models.map(model=><div key={model.id}>{model.name} · {model.model} · {model.kind==="test"?tr("Testanbieter","Test provider"):model.endpoint||model.kind}</div>)}</dd>
      <dt>{tr("Prozessnetzwerk","Process network")}</dt><dd>{network?.kind==="web_proxy"?tr("Verwalteter Web-Proxy: ","Managed web proxy: ")+network.policy_id:network?.kind==="denied"?tr("Gesperrt","Denied"):network?.kind==="user_rights"?tr("Netzwerkrechte deines Benutzerkontos","Network permissions of your user account"):tr("Keine allgemeinen Prozesse","No general processes")}</dd>
      <dt>{tr("Dateizugriff","File access")}</dt><dd>{boundary?.folders.length?boundary.folders.map(folder=><div key={folder.id}>{folder.name} · {folder.writable?tr("Lesen und Schreiben","Read and write"):tr("Nur Lesen","Read only")}</div>):tr("Keine freigegebenen Ordner","No granted folders")}</dd>
      <dt>{tr("Freigabemodus","Approval mode")}</dt><dd>{flight?.access_mode==="autonomous"?tr("Erlaubte Aktionen automatisch; native Aktionen benötigen eine eigene Freigabe.","Allowed actions are automatic; native actions require separate approval."):flight?.access_mode==="read_only"?tr("Nur Lesen","Read only"):flight?.access_mode==="confirm_all"?tr("Jede Werkzeugaktion bestätigen","Confirm every tool action"):tr("Verändernde Aktionen bestätigen","Confirm changes")}</dd>
      <dt>{tr("Grenzen","Limits")}</dt><dd>{budget?.max_seconds} s · {budget?.max_steps} {tr("Schritte","steps")} · {budget?.max_total_tokens??defaultRunBudgets.max_total_tokens} {tr("reservierte Tokens höchstens","maximum reserved tokens")}</dd>
    </dl>
    {flight&&"workflow" in flight&&flight.workflow.inputs!==undefined&&<section className="product-item"><h4>{tr("Gebundene Workflow-Eingaben","Bound workflow inputs")}</h4><WorkflowDataValuePreview value={flight.workflow.inputs}/></section>}
    <details><summary>{tr("Erlaubte Werkzeuge anzeigen","Show allowed tools")}</summary><p>{tr("Dienstprofil: ","Service profile: ")}{boundary?.executor}</p><ul>{flight?.allowed_tools.map(tool=><li key={tool}>{tool}</li>)}</ul></details>
    {!!boundary?.connector_aliases.length&&<p>{tr("Zusätzliche Connectoren: ","Additional connectors: ")}{boundary.connector_aliases.join(", ")}</p>}
    <p className="muted">{tr("Modell- und Connectoranfragen können Inhalte übertragen, auch wenn das Prozessnetzwerk gesperrt ist.","Model and connector requests can transmit content even when process networking is denied.")}</p>
    {!boundary?.input_snapshot_bound&&<p className="muted">{tr("Dateien werden über die angezeigten Ordnerfreigaben gelesen. Dieser Auftrag verwendet keine eingefrorene Dateikopie.","Files are read through the folder grants shown above. This task does not use a frozen file copy.")}</p>}
    {pending&&<p role="status">{tr("Die ursprüngliche Bestätigung ist gespeichert. Kläre denselben Vorgang; ein neuer Lauf wird nicht erzeugt.","The original confirmation is saved. Reconcile the same operation; no new run is created.")}</p>}
    <div className="actions"><button type="button" className="primary" disabled={busy||confirmation.invalid||prepared.retired} onClick={()=>void commit()}>{pending?tr("Ursprünglichen Start abgleichen","Reconcile original start"):prepared.committed_run_id?tr("Bestätigten Start anzeigen","Show accepted start"):tr("Diesen Plan bestätigen und starten","Confirm this plan and start")}</button>
    <button type="button" disabled={busy||confirmation.invalid} onClick={()=>void retire()}>{tr("Plan verbindlich verwerfen","Retire this plan")}</button></div></>}
  </section>;
}
