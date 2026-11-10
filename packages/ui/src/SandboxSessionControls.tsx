import {useEffect,useRef,useState} from "react";
import type {Client} from "@cowork/client";
import type {EnvironmentReference} from "./EnvironmentPanel";
import {SandboxSessionLatch,manageSandboxSession,inspectSandboxSession,sandboxExpectedUid,createSandboxPreview,sandboxOperationHistory,unresolvedSandboxOperation,definitiveSandboxRejection,type SandboxOperation,type SandboxSessionReceipt} from "./sandboxSessionApi";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {useView} from "./view";

export function SandboxSessionControls(props:{client:Client,project:string,order:string,environment:EnvironmentReference,disabled?:boolean}){
  return <SessionControls key={JSON.stringify([props.client.base,props.project,props.order,props.environment])} {...props}/>;
}
function SessionControls({client,project,order,environment,disabled=false}:{client:Client,project:string,order:string,environment:EnvironmentReference,disabled?:boolean}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [latch]=useState(()=>new SandboxSessionLatch(workspaceJournalKey(client,JSON.stringify(["sandbox-session",project,order,environment]))));
  const [session,setSession]=useState<SandboxSessionReceipt|null>(null),[sessionObserved,setSessionObserved]=useState(false),[busy,setBusy]=useState(false),[pending,setPending]=useState(latch.pending),[error,setError]=useState("");
  const inFlight=useRef(false);
  const historyChecked=useRef(false);
  const recoveryEpoch=useRef(0),mounted=useRef(true);
  const [recovering,setRecovering]=useState(!latch.pending),[recoveryBlocked,setRecoveryBlocked]=useState(false);
  async function recover(){
    if(inFlight.current)return;
    inFlight.current=true;historyChecked.current=false;
    const epoch=++recoveryEpoch.current;setRecovering(true);setRecoveryBlocked(false);setSessionObserved(false);setPreview(null);setError("");
    try{const history=await sandboxOperationHistory(client,project,order);if(!mounted.current||epoch!==recoveryEpoch.current)return;
      if(latch.pending&&!latch.invalid){const original=latch.prepare(()=>{throw new Error("sandbox_original_request_missing");});if(definitiveSandboxRejection(history,original)){latch.confirm();setPending(false);}}
      if(!latch.pending){const original=unresolvedSandboxOperation(history,environment);if(original){latch.prepare(()=>original);setPending(true);}}
      historyChecked.current=true;
      const current=await inspectSandboxSession(client,project,order,environment);if(!mounted.current||epoch!==recoveryEpoch.current)return;
      setSession(current);setSessionObserved(true);
    }
    catch(reason){if(mounted.current&&epoch===recoveryEpoch.current){setRecoveryBlocked(true);setError(reason instanceof Error?reason.message:String(reason));}}
    finally{if(mounted.current&&epoch===recoveryEpoch.current){inFlight.current=false;setRecovering(false);}}
  }
  useEffect(()=>{mounted.current=true;void recover();return()=>{mounted.current=false;recoveryEpoch.current++;inFlight.current=false;};},[]);
  const [preview,setPreview]=useState<{url:string,expires_at:number}|null>(null);
  async function issuePreview(){
    if(inFlight.current||!session?.sandbox_uid)return;
    inFlight.current=true;setBusy(true);setError("");setPreview(null);
    try{const origin=client.base||window.location.origin;setPreview(await createSandboxPreview(client,project,order,environment,session.sandbox_uid,origin));}
    catch(reason){setError(reason instanceof Error?reason.message:String(reason));}
    finally{inFlight.current=false;setBusy(false);}
  }
  async function operation(next:SandboxOperation,reconcile=false){
    if(inFlight.current||next!=="inspect"&&(disabled||recovering||!reconcile&&(recoveryBlocked||!sessionObserved)))return;
    inFlight.current=true;setBusy(true);setError("");
    try{
      if(next==="inspect"){
        setSessionObserved(false);setPreview(null);
        const current=await inspectSandboxSession(client,project,order,environment);
        setSession(current);setSessionObserved(true);if(historyChecked.current)setRecoveryBlocked(false);
        return;
      }
      const build=()=>({environment:structuredClone(environment),idempotency_key:crypto.randomUUID(),operation:next,expected_sandbox_uid:sandboxExpectedUid(next,session,sessionObserved)});
      // Observation has its own operation key and cannot replace or settle an
      // earlier mutation whose acknowledgement was lost.
      const request=latch.prepare(()=>{if(reconcile)throw new Error("sandbox_original_request_missing");return build();});
      setPending(true);setPreview(null);
      const receipt=await manageSandboxSession(client,project,order,request);
      latch.confirm();setPending(false);
      setSession(receipt);setSessionObserved(!reconcile);
      // A cached original receipt may describe an earlier physical session.
      // Observe again before it can authorize a new lifecycle mutation.
      if(reconcile){const current=await inspectSandboxSession(client,project,order,environment);setSession(current);setSessionObserved(true);if(historyChecked.current)setRecoveryBlocked(false);}
    }catch(reason){setPending(latch.pending);setError(reason instanceof Error?reason.message:String(reason));}
    finally{inFlight.current=false;setBusy(false);}
  }
  const labels:Record<SandboxOperation,string>={ensure:tr("Umgebung starten","Start environment"),inspect:tr("Status prüfen","Inspect status"),suspend:tr("Pausieren","Suspend"),resume:tr("Fortsetzen","Resume"),stop:tr("Stoppen","Stop")};
  return <section className="sandbox-session-controls"><h4>{tr("Kata-Sitzung für diesen Chat","Kata session for this chat")}</h4>
    <p className="muted">{tr("Arbeitsdateien bleiben getrennt vom laufenden Prozess erhalten. Stoppen beendet die Laufzeit; Löschen von Projektdaten erfolgt separat.","Working files persist separately from running processes. Stop ends the runtime; project data deletion is separate.")}</p>
    <div className="actions">{(["ensure","suspend","resume","stop"] as SandboxOperation[]).map(name=><button type="button" key={name} disabled={disabled||busy||pending||recovering||recoveryBlocked||!sessionObserved||name!=="ensure"&&!session?.sandbox_uid} onClick={()=>void operation(name)}>{labels[name]}</button>)}<button type="button" disabled={busy||recovering} onClick={()=>void operation("inspect")}>{labels.inspect}</button><button type="button" disabled={busy||recovering} onClick={()=>void recover()}>{tr("Offene Vorgänge wiederherstellen","Recover pending operations")}</button><button type="button" disabled={busy||recovering||!sessionObserved||!session?.sandbox_uid||!["ready","suspended"].includes(session?.state??"")} onClick={()=>void issuePreview()}>{tr("Vorschau-Link erstellen","Create preview link")}</button></div>
    {recovering&&<p role="status">{tr("Offene ursprüngliche Vorgänge werden geprüft …","Checking unresolved original operations …")}</p>}
    {!recovering&&!sessionObserved&&<p role="status">{tr("Der Sitzungsstatus ist unbekannt. Vor neuen Änderungen den Status prüfen; eine offene ursprüngliche Anfrage kann separat abgeglichen werden.","Session status is unknown. Inspect before new changes; an unresolved original request can be reconciled separately.")}</p>}
    {!recovering&&sessionObserved&&!session&&<p role="status">{tr("Für diesen Chat besteht noch keine Sitzung. Du kannst eine neue Umgebung starten.","This chat has no session yet. You can start a new environment.")}</p>}
    {preview&&<p><a href={preview.url} target="_blank" rel="noopener noreferrer">{tr("Vorschau separat öffnen","Open preview separately")}</a> · {tr("Gültig bis","Valid until")} {new Date(preview.expires_at).toLocaleTimeString(language)}</p>}
    {pending&&<div role="status"><p>{latch.invalid?tr("Der gespeicherte Vorgang ist ungültig. Neue Änderungen sind gesperrt.","The saved operation is invalid. New changes are blocked."):tr("Die Bestätigung fehlt. Neue Änderungen sind gesperrt; Abgleichen verwendet exakt dieselbe ursprüngliche Anfrage.","The acknowledgement is missing. New changes are blocked; reconciliation uses the exact original request.")}</p><button type="button" disabled={busy||disabled||latch.invalid||recovering} onClick={()=>void operation("ensure",true)}>{tr("Ursprüngliche Anfrage abgleichen","Reconcile original request")}</button></div>}
    {session&&<p role="status">{session.state} · {tr("Arbeitsdateien erhalten","Working files preserved")}: {session.workspace_preserved?tr("ja","yes"):tr("nein","no")} · {new Date(session.observed_at).toLocaleString(language)}</p>}
    {error&&<p role="alert">{error}</p>}
  </section>;
}
