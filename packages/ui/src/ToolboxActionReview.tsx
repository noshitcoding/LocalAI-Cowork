import {useEffect,useRef,useState} from "react";
import type {Client} from "@cowork/client";
import {productRequest,type ToolJob,type ToolPreview,type ToolRequest} from "./productApi";
import {newToolboxRequest,toolboxCapture,toolboxOutcome} from "./toolboxWorkbench";
import {useView} from "./view";
import {readPendingToolbox,savePendingToolbox,clearPendingToolbox,recoverPendingToolbox,submitPendingToolbox,type PendingToolboxAction,type ToolboxComponent} from "./toolboxPending";

export function useToolboxAction(client:Client,order:string,component:ToolboxComponent){
  function load(){try{return {record:typeof localStorage==="undefined"?null:readPendingToolbox(localStorage,client.base,order,component),error:""};}catch{return {record:null,error:"pending_toolbox_journal_invalid"};}}
  const [initial]=useState(load),[request,setRequest]=useState<ToolRequest|null>(initial.record?.request??null),[preview,setPreview]=useState<ToolPreview|null>(initial.record?.preview??null),[job,setJob]=useState<ToolJob|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(initial.error),[pending,setPending]=useState(!!initial.record),[journalBlocked,setJournalBlocked]=useState(!!initial.error);
  const pendingRecord=useRef<PendingToolboxAction|null>(initial.record);
  const path=`/orders/${encodeURIComponent(order)}/toolbox`;
  const scope=useRef({client,order,component,generation:0});if(scope.current.client!==client||scope.current.order!==order||scope.current.component!==component)scope.current={client,order,component,generation:scope.current.generation+1};
  useEffect(()=>{const loaded=load();pendingRecord.current=loaded.record;setRequest(loaded.record?.request??null);setPreview(loaded.record?.preview??null);setPending(!!loaded.record);setJournalBlocked(!!loaded.error);setJob(null);setBusy(false);setError(loaded.error);},[client,order,component]);
  useEffect(()=>{
    if(!job||job.state!=="running")return;let live=true;
    const interval=setInterval(()=>void productRequest<ToolJob>(client,`${path}/jobs/${job.id}`).then(value=>{if(live)setJob(value);},error=>{if(live)setError(error instanceof Error?error.message:String(error));}),1000);
    return()=>{live=false;clearInterval(interval);};
  },[client,path,job?.id,job?.state]);
  async function prepare(action:ToolRequest["action"]){
    if(busy||pendingRecord.current||journalBlocked||job?.state==="running")return;setBusy(true);setError("");setRequest(null);setPreview(null);setJob(null);
    const request=newToolboxRequest(action),generation=scope.current.generation;
    try{const preview=await productRequest<ToolPreview>(client,path+"/preview","POST",request);if(scope.current.generation===generation){setRequest(request);setPreview(preview);}}catch(error){if(scope.current.generation===generation)setError(error instanceof Error?error.message:String(error));}finally{if(scope.current.generation===generation)setBusy(false);}
  }
  async function execute(){
    if(!request||!preview||busy||journalBlocked||job?.state==="running")return;setBusy(true);setError("");
    const generation=scope.current.generation;
    let original=pendingRecord.current;
    try{
      if(typeof localStorage==="undefined")throw new Error("pending_toolbox_storage_unavailable");
      if(!original){original=savePendingToolbox(localStorage,client.base,order,component,request,preview);pendingRecord.current=original;setPending(true);}
      const saved=await submitPendingToolbox(client,original);
      clearPendingToolbox(localStorage,original);
      if(scope.current.generation===generation){pendingRecord.current=null;setPending(false);setJob(saved);}
    }catch(error){
      if(scope.current.generation===generation)setError(error instanceof Error?error.message:String(error));
      if(!original&&error instanceof Error&&error.message==="pending_toolbox_action_exists"){try{const existing=readPendingToolbox(localStorage,client.base,order,component);if(existing){pendingRecord.current=existing;setRequest(existing.request);setPreview(existing.preview);setPending(true);}}catch{setJournalBlocked(true);}}
      if(original&&error instanceof Error&&/\(4\d\d\)$/.test(error.message)){
        // A previous lost response can coexist with a later configuration rejection.
        // Only clear after a fresh authenticated listing confirms there is no original job.
        try{const recovered=await recoverPendingToolbox(client,original);clearPendingToolbox(localStorage,original);if(scope.current.generation===generation){pendingRecord.current=null;setPending(false);if(recovered){setJob(recovered);setError("");}else{setRequest(null);setPreview(null);}}}catch{/* Preserve the durable original until reconciliation succeeds. */}
      }
    }finally{if(scope.current.generation===generation)setBusy(false);}
  }
  async function cancel(){if(!job||busy)return;setBusy(true);try{setJob(await productRequest<ToolJob>(client,`${path}/jobs/${job.id}/cancel`,"POST",{}));}catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}}
  async function recover(){const original=pendingRecord.current;if(!original||busy)return;setBusy(true);setError("");try{const saved=await recoverPendingToolbox(client,original);if(!saved){setError("pending_toolbox_not_confirmed");return;}clearPendingToolbox(localStorage,original);pendingRecord.current=null;setPending(false);setJob(saved);}catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}}
  function reset(){if(busy||pendingRecord.current||journalBlocked||job?.state==="running")return;setRequest(null);setPreview(null);setJob(null);setError("");}
  function invalidatePreview(){if(busy||pendingRecord.current||journalBlocked||job)return;setRequest(null);setPreview(null);}
  return {request,preview,job,busy,error,pending,journalBlocked,prepare,execute,cancel,recover,reset,invalidatePreview,locked:busy||pending||journalBlocked||job?.state==="running"};
}
export type ToolboxActionController=ReturnType<typeof useToolboxAction>;
export function ToolboxActionReview({action,hideOutput=false}:{action:ToolboxActionController,hideOutput?:boolean}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en,outcome=toolboxOutcome(action.job);
  return <section className="toolbox-action-review">{action.pending&&<p role="status">{tr("Eine frühere Antwort ist unklar. Neue Aktionen bleiben gesperrt, bis der gespeicherte Vorgang abgeglichen ist. Der ursprüngliche Vorgang wurde nach dem Neuladen wiederhergestellt und wird nicht automatisch ausgeführt.","A previous response is uncertain. New actions stay locked until the saved operation is reconciled. The original operation was restored after reload and is never executed automatically.")}</p>}{action.journalBlocked&&<p role="alert">{tr("Der ausstehende Vorgang konnte nicht sicher gelesen werden. Es wird keine neue Aktion gestartet.","The pending operation could not be read safely. No new action will be started.")}</p>}{action.error&&<p role="alert" className="error">{action.error}</p>}{action.preview&&action.request&&!action.job&&<article className="settings-card"><h3>{action.pending?tr("Ursprüngliche Aktion abgleichen","Reconcile original action"):tr("Aktion prüfen","Review action")}</h3><pre>{JSON.stringify(action.request.action,null,2)}</pre><p><code>{action.preview.cwd}</code> · {action.preview.timeout_seconds}s</p><button className="primary" type="button" disabled={action.busy||action.journalBlocked} onClick={()=>void action.execute()}>{action.pending?tr("Genau diesen Vorgang erneut bestätigen","Retry this exact operation"):tr("Diese Aktion ausführen","Execute this action")}</button>{action.pending&&<button type="button" disabled={action.busy} onClick={()=>void action.recover()}>{tr("Gespeicherten Status abgleichen","Reconcile saved status")}</button>}<p className="muted">{tr("Ein erneuter Versuch verwendet denselben Vorgang. Eine unklare Ausführung wird nicht automatisch wiederholt.","Retrying uses the same operation. An uncertain execution is never repeated automatically.")}</p></article>}{action.job&&<article className="settings-card"><header><h3>{tr("Werkzeugausgabe","Tool output")}</h3><span>{action.job.state}{outcome?` · ${outcome.termination} · ${tr("Exit-Code","Exit code")} ${outcome.exit_code??"—"}`:""}</span></header>{action.job.state==="running"&&<button type="button" disabled={action.busy} onClick={()=>void action.cancel()}>{tr("Abbrechen","Cancel")}</button>}{action.job.error&&<p role="alert">{action.job.error}</p>}{outcome&&!hideOutput&&<><pre>{toolboxCapture(outcome.stdout)}</pre>{outcome.stderr.bytes.length>0&&<details open={outcome.exit_code!==0}><summary>{tr("Fehlerausgabe","Standard error")}</summary><pre>{toolboxCapture(outcome.stderr)}</pre></details>}{(outcome.stdout.truncated||outcome.stderr.truncated||!outcome.output_complete)&&<p role="alert">{tr("Die Ausgabe ist unvollständig.","Output is incomplete.")}</p>}</>}</article>}</section>;
}
