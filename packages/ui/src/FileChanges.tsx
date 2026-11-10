import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {Client, type FileChange, type FileOperation, type RunDetail, type RunEvent, type RestoreFile} from "@cowork/client";
import type {Key} from "./i18n";
type T=(key:Key)=>string;

function VersionPair({change,client,t}:{change:FileChange,client:Client,t:T}) {
  const before=useQuery({queryKey:["file-version",change.run_id,change.tool_call_id,"before"],queryFn:()=>client.fileVersion(change.run_id,change.tool_call_id,true),enabled:!!change.before_sha256});
  const after=useQuery({queryKey:["file-version",change.run_id,change.tool_call_id,"after"],queryFn:()=>client.fileVersion(change.run_id,change.tool_call_id,false)});
  return <div className="file-versions">
    {[{title:t("previousVersion"),query:before,absent:!change.before_sha256},{title:t("proposedVersion"),query:after,absent:false}].map(({title,query,absent})=><section key={title}><h4>{title}</h4>
      {absent?<p>{t("newFile")}</p>:query.isError?<p role="alert">{t("versionUnavailable")} <button onClick={()=>query.refetch()}>{t("retry")}</button></p>:query.data?<pre>{query.data.content}</pre>:<p role="status">{t("loading")}</p>}
    </section>)}
  </div>;
}
function ChangeView({change,client,t,restore}:{change:FileChange,client:Client,t:T,restore?:(target:RestoreFile)=>void}) {
  const [show,setShow]=useState(false);
  return <article className="file-change"><strong>{change.path}</strong><small>{new Intl.NumberFormat().format(change.size)} B</small>
    <button className="quiet" aria-expanded={show} onClick={()=>setShow(!show)}>{t("compareVersions")}</button>
    {show&&<VersionPair change={change} client={client} t={t}/>}
    {restore&&<div className="actions">{change.before_sha256&&<button onClick={()=>restore({expected_source_sha256:change.before_sha256,folder_id:change.folder_id,path:change.path+".restored",source:{run_id:change.run_id,tool_call_id:change.tool_call_id,version:"text_before"}})}>{t("restorePrevious")}</button>}
      <button onClick={()=>restore({expected_source_sha256:change.after_sha256,folder_id:change.folder_id,path:change.path+".restored",source:{run_id:change.run_id,tool_call_id:change.tool_call_id,version:"text_after"}})}>{t("restoreWritten")}</button></div>}
    <details><summary>{t("artifactIntegrity")}</summary><dl><dt>{t("previousVersion")}</dt><dd><code>{change.before_sha256??t("newFile")}</code></dd><dt>{t("proposedVersion")}</dt><dd><code>{change.after_sha256}</code></dd></dl></details>
  </article>;
}
function OperationView({operation,t,restore,restoreInCopy=false}:{operation:FileOperation,t:T,restore?:(target:RestoreFile)=>void,restoreInCopy?:boolean}) {
  const key=operation.kind==="create_directory"?"create_directory":operation.kind==="rename"?"rename_file":operation.kind==="restore"?"restore_file":operation.kind==="import"?"import_file":operation.kind==="generate"?"generatedFile":"remove_file";
  return <article className="file-change"><h4>{t(key)}</h4><strong>{operation.path}</strong>{operation.destination&&<p>{t("destinationPath")}: {operation.destination}</p>}
    {operation.kind==="remove"&&<p>{t("removeFileHint")}</p>}
    {operation.imported_from&&<p>{t("uploadedSource")}: {operation.imported_from.file.file_name} · {operation.imported_from.file.size} B</p>}
    {operation.generated_from&&<p>{t("generatedBy")}: {t((operation.generated_from.tool in {document_write:1,image_edit:1}?operation.generated_from.tool:"details") as Key)} · {operation.generated_from.operation}{!!operation.generated_from.inputs.length&&<> · {t("documentInputs")}: {operation.generated_from.inputs.map(i=>i.path).join(", ")}</>}</p>}
    {restore&&<button onClick={()=>restore({expected_source_sha256:(operation.before??operation.after)?.sha256,folder_id:operation.folder_id,path:operation.kind==="remove"&&!restoreInCopy?operation.path:operation.path+".restored",source:{run_id:operation.run_id,tool_call_id:operation.tool_call_id,version:operation.before?"operation_before":"operation_after"}})}>{t("restore_file")}</button>}
    <details><summary>{t("artifactIntegrity")}</summary><dl><dt>{t("previousVersion")}</dt><dd><code>{operation.before?.sha256??"—"}</code></dd><dt>{t("proposedVersion")}</dt><dd><code>{operation.after?.sha256??"—"}</code></dd></dl>
      {!!operation.entries.length&&<><p>{t("retainedFiles")} · {operation.entries.length}</p><ul>{operation.entries.map(e=><li key={e.path}>{e.path} · {e.size} B</li>)}</ul></>}
      {operation.restored_from&&<pre>{JSON.stringify(operation.restored_from,null,2)}</pre>}
      {operation.imported_from&&<pre>{JSON.stringify(operation.imported_from,null,2)}</pre>}
      {operation.generated_from&&<pre>{JSON.stringify(operation.generated_from,null,2)}</pre>}
    </details></article>;
}
export function FileChanges({detail,events,client,t,busy,action,restore,copyRestore=false}:{detail:RunDetail,events:RunEvent[],client:Client,t:T,busy:boolean,action:(fn:()=>Promise<unknown>)=>Promise<void>,restore:(target:RestoreFile)=>void,copyRestore?:boolean}) {
  const fingerprint=detail.uncertain_file_fingerprint;
  // Copy receipts require the confirmed-copy planner. Native-only consumers
  // must not route them to the legacy original-folder restoration endpoint.
  const availableRestore=detail.run.spec.input_workspace&&!copyRestore?undefined:restore;
  const changes=detail.file_changes??[];
  const operations=detail.file_operations??[];
  // Reads can straddle a receipt commit. Do not present the same action as both confirmed and pending.
  const uncertain=changes.some(c=>c.tool_call_id===detail.uncertain_file_change?.tool_call_id)?null:detail.uncertain_file_change;
  const pendingOperation=operations.some(o=>o.tool_call_id===detail.uncertain_file_operation?.tool_call_id)?null:detail.uncertain_file_operation;
  const sequence=new Map<string,number>();
  for(const event of events){const payload=event.payload as Record<string,unknown>;
    if(typeof payload.tool_call_id==="string"&&(event.kind==="tool_completed"||(["file_write_reconciled","file_operation_reconciled"].includes(event.kind)&&payload.accepted_completed===true)))sequence.set(payload.tool_call_id,event.sequence);
  }
  const receipts=[...changes.map(change=>({id:change.tool_call_id,view:<ChangeView key={change.tool_call_id} change={change} client={client} t={t} restore={availableRestore}/>})),
    ...operations.map(operation=>({id:operation.tool_call_id,view:<OperationView key={operation.tool_call_id} operation={operation} t={t} restore={availableRestore} restoreInCopy={copyRestore}/>}))]
    .sort((a,b)=>(sequence.get(a.id)??Number.MAX_SAFE_INTEGER)-(sequence.get(b.id)??Number.MAX_SAFE_INTEGER));
  if(!uncertain&&!changes.length&&!pendingOperation&&!operations.length)return null;
  return <section className="file-changes"><h2>{t("fileChanges")}</h2>
    {!availableRestore&&receipts.length>0&&<p className="muted">{t("runCopyRestoreUnavailable")}</p>}
    {availableRestore&&copyRestore&&receipts.length>0&&<p className="muted">{t("runCopyRestoreHint")}</p>}
    {receipts.map(receipt=>receipt.view)}
    {(uncertain||pendingOperation)&&detail.run.state!=="interrupted"&&<p role="status">{t("writeInProgress")}</p>}
    {(uncertain||pendingOperation)&&detail.run.state==="interrupted"&&<section className="approval"><h3>{t("uncertainWrite")}</h3><p>{t("uncertainHint")}</p>{uncertain&&<ChangeView change={uncertain} client={client} t={t}/>} {pendingOperation&&<OperationView operation={pendingOperation} t={t}/>}
      {detail.run.state==="interrupted"&&fingerprint?<><p>{t("reconcileHint")}</p><div className="actions">
        <button disabled={busy} onClick={()=>void action(()=>client.reconcileFile(detail.run.id,{fingerprint,decision:"confirm_not_applied"}))}>{t("confirmNotApplied")}</button>
        <button disabled={busy} onClick={()=>void action(()=>client.reconcileFile(detail.run.id,{fingerprint,decision:"accept_completed"}))}>{t("acceptCompleted")}</button>
      </div></>:<p role="status">{t("writeInProgress")}</p>}
    </section>}
  </section>;
}
