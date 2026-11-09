import {useEffect,useMemo,useRef,useState,type ReactNode} from "react";
import {RequestError,type Client,type WorkflowDefinition,type WorkflowOperation} from "@cowork/client";
import type {Key} from "./i18n";
import {WorkflowSnapshotReader,workflowSnapshotIdentity,type WorkflowSnapshotRead} from "./workflowSnapshotRecovery";

type Props={client:Client;project:string;operation:WorkflowOperation;busy:boolean;blocked:boolean;copied:boolean;t:(key:Key)=>string;
  onCopy:(value:WorkflowSnapshotRead)=>Promise<void>;onExport:(value:unknown)=>Promise<void>;summary:(definition:WorkflowDefinition)=>ReactNode};
export function WorkflowSnapshotRecovery({client,project,operation,busy,blocked,copied,t,onCopy,onExport,summary}:Props){
  const identity=workflowSnapshotIdentity(operation);
  const reader=useMemo(()=>new WorkflowSnapshotReader(client,project,operation),[client,project,identity,operation.state]);
  const active=useRef<WorkflowSnapshotReader|null>(reader);active.current=reader;
  useEffect(()=>{reader.activate();active.current=reader;return()=>{reader.dispose();if(active.current===reader)active.current=null;};},[reader]);
  const [selection,setSelection]=useState<{reader:WorkflowSnapshotReader;value:WorkflowSnapshotRead|null;loading:boolean;error:unknown}>({reader,value:null,loading:false,error:null});
  const current=selection.reader===reader?selection:{reader,value:null,loading:false,error:null};
  async function read(copy=false){
    if(busy||current.loading||operation.state==="pending"||(copy&&(blocked||copied)))return;
    setSelection({...current,reader,loading:true,error:null});
    try{const value=await reader.read();if(!value||active.current!==reader)return;
      setSelection({reader,value,loading:false,error:null});
      if(copy&&value.reason===null)await onCopy(value);
    }catch(error){if(active.current===reader)setSelection(old=>({...old,reader,loading:false,error}));}
  }
  if(operation.state==="pending"||!operation.draft_snapshot||(operation.request.kind!=="create"&&operation.request.kind!=="publish"))return null;
  const value=current.value;
  return <section className="workflow-snapshot-recovery" aria-label={t("workflowSnapshotTitle")}>
    <h4>{t("workflowSnapshotTitle")}</h4><p>{t("workflowSnapshotHint")}</p>
    <button type="button" disabled={busy||current.loading} onClick={()=>void read()}>{t(current.loading?"loading":"workflowSnapshotPreview")}</button>
    {value&&<><p role="status">{t(value.reason==="pending"?"workflowSnapshotPending":value.reason==="binding"?"workflowSnapshotBinding":value.reason==="unsupported"?"workflowDraftUnsupported":"workflowSnapshotVerified")}</p>
      {value.definition&&summary(value.definition)}
      {value.version&&<p>{t("workflowSnapshotOriginalVersion")}: {value.version.reference.name} · {t("workflowVersion")} {value.version.reference.revision}</p>}
      <div className="actions"><button type="button" disabled={busy||current.loading||blocked||copied||value.reason!==null} onClick={()=>void read(true)}>{t(copied?"workflowSnapshotCopied":"workflowSnapshotCopy")}</button>
        {value.snapshot&&<button type="button" disabled={busy||current.loading} onClick={()=>void onExport({operation:value.operation,snapshot:value.snapshot,original_version:value.version})}>{t("workflowDraftExport")}</button>}</div>
      {value.snapshot&&<details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(value.snapshot,null,2)}</pre></details>}
    </>}
    {!!current.error&&<p role="alert">{t(current.error instanceof RequestError&&current.error.details.code==="budget_exceeded"?"workflowJournalQuota":"workflowSnapshotUnavailable")}</p>}
    {copied&&<p role="status">{t("workflowSnapshotCopyHint")}</p>}
  </section>;
}
