import {useEffect,useRef,useState} from "react";
import {Client,type ProcessAttempt,type RunDetail} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {useView} from "./view";
import {decodeManagedProcessIntent,originalManagedProcessRecheckAvailable,submitManagedProcessReconciliation} from "./managedProcessReconciliation";

const connections=new WeakMap<Client,{base:string;token:string;id:number}>();
let nextConnection=1;
function connectionId(client:Client) {
  const prior=connections.get(client);
  if(prior?.base===client.base&&prior.token===client.token)return prior.id;
  const id=nextConnection++;connections.set(client,{base:client.base,token:client.token,id});return id;
}
export function managedProcessRecoveryAvailable(detail:RunDetail):boolean {
  return !!detail.run.spec.input_workspace&&!!detail.run.spec.environment&&!detail.run.spec.workflow;
}
type Props={client:Client;order:string;detail:RunDetail;busy:boolean;fresh:boolean;changed:()=>Promise<void>};
export function ManagedProcessRecoveryPanel(props:Props) {
  const attempts=props.detail.process_attempts??[];
  if(props.detail.run.spec.order_id!==props.order||!managedProcessRecoveryAvailable(props.detail)||!attempts.length)return null;
  const connection=connectionId(props.client);
  return <section className="working-files" aria-label="Runtime process status">
    {attempts.map(attempt=><Attempt key={JSON.stringify([connection,props.order,props.detail.run.id,attempt.id])} {...props} attempt={attempt}/>)}
  </section>;
}
function Attempt({client,order,detail,attempt,busy,fresh,changed}:Props&{attempt:ProcessAttempt}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [journal]=useState(()=>new PersistentRequestLatch(workspaceJournalKey(client,`managed-process:${JSON.stringify([order,detail.run.id,attempt.id])}`),decodeManagedProcessIntent));
  const active=useRef(true),pending=useRef(false);
  const [saving,setSaving]=useState(false),[failed,setFailed]=useState(false),[uncertain,setUncertain]=useState(journal.pending);
  useEffect(()=>{active.current=true;return()=>{active.current=false;};},[]);
  const state=attempt.state;
  const physical=state.state==="completed"||state.state==="reconciled_completed"?state.result:null;
  const ready=fresh&&!busy&&!saving&&!journal.invalid;
  const available=ready&&(journal.pending?originalManagedProcessRecheckAvailable(journal,detail.run,attempt):detail.run.state==="interrupted"&&state.state==="launch_committed");
  function discard() {
    if(!fresh||busy||saving||!window.confirm(tr("Nur die lokale Abgleichreferenz entfernen? Der gespeicherte Prozess und ein bereits bestätigter Abschluss bleiben erhalten.","Remove only the local reconciliation reference? The retained process and any confirmed completion remain unchanged.")))return;
    try {journal.discard();setUncertain(false);setFailed(false);}catch{setFailed(true);}
  }
  async function reconcile(finishCancellation:boolean) {
    if(!active.current||pending.current||!available)return;
    pending.current=true;setSaving(true);setFailed(false);
    try {
      await submitManagedProcessReconciliation(client,order,detail.run.id,attempt.id,journal,()=>({expected_run_revision:detail.run.revision,
        expected_request_sha256:attempt.request_sha256,finish_cancellation:finishCancellation}));
      if(active.current)setUncertain(false);
    } catch {if(active.current){setFailed(true);setUncertain(journal.pending);}}
    finally {
      if(active.current){try{await changed();}catch{if(active.current)setFailed(true);}if(active.current)setSaving(false);}
      pending.current=false;
    }
  }
  return <article>
    <h3>{tr("Status des isolierten Prozesses","Isolated process status")}</h3>
    {physical?<p>{tr("Belegtes Prozessende","Verified process termination")}: {physical.termination} · {tr("Exit-Code","Exit code")}: {physical.exit_code??tr("nicht bekannt","unknown")}</p>:state.state==="launch_committed"&&<p>{tr("Das tatsächliche Prozessende ist noch nicht bestätigt.","Actual process termination is not confirmed yet.")}</p>}
    {state.state==="reconciled_not_started"&&<p>{tr("Der ursprüngliche Versuch wurde nachweislich nicht gestartet.","The original attempt was verified as not started.")}</p>}
    {state.state==="launch_committed"&&detail.run.state==="interrupted"&&<>
      <p>{tr("Prüfe die ursprüngliche Abschlussquittung. Danach kannst du den Auftrag fortsetzen oder ihn zugleich als abgebrochen abschließen.","Check the original completion receipt. You can then continue the task or close it as canceled in the same step.")}</p>
      {!uncertain&&<>
        <button disabled={!available} onClick={()=>void reconcile(false)}>{tr("Prozessabschluss prüfen","Check process completion")}</button>
        <button disabled={!available} onClick={()=>void reconcile(true)}>{tr("Prozessabschluss prüfen und Auftrag abbrechen","Check process completion and cancel task")}</button>
      </>}
    </>}
    {uncertain&&!journal.invalid&&<><p role="status">{tr("Die Bestätigung der ursprünglichen Abgleichanfrage ist noch offen.","Confirmation of the original reconciliation request is pending.")}</p><button disabled={!available} onClick={()=>void reconcile(false)}>{tr("Ursprünglichen Abgleich erneut prüfen","Recheck original reconciliation")}</button></>}
    {saving&&<p role="status">{tr("Ursprünglicher Prozessabschluss wird geprüft …","Checking original process completion …")}</p>}
    {(failed||journal.invalid)&&<p role="alert">{journal.invalid?tr("Die gespeicherte Abgleichanfrage ist nicht lesbar. Es wird keine neue Anfrage gesendet.","The retained reconciliation request is unreadable. No new request will be sent."):tr("Der Abgleich wurde nicht bestätigt. Aktualisiere den gespeicherten Stand oder prüfe dieselbe Anfrage ausdrücklich erneut.","Reconciliation was not confirmed. Refresh the retained state or explicitly recheck the same request.")}</p>}
    {(uncertain||journal.invalid)&&<button disabled={!fresh||busy||saving} onClick={discard}>{tr("Lokale Abgleichreferenz entfernen","Remove local reconciliation reference")}</button>}
    {(failed||uncertain||journal.invalid)&&<button disabled={busy||saving} onClick={()=>void changed().catch(()=>{if(active.current)setFailed(true);})}>{tr("Gespeicherten Stand aktualisieren","Refresh retained state")}</button>}
    {physical&&detail.run.state==="canceled"&&<p>{tr("Der Auftrag ist als abgebrochen abgeschlossen. Das belegte Prozessende bleibt separat erhalten.","The task is closed as canceled. Its verified physical process result is retained separately.")}</p>}
  </article>;
}
