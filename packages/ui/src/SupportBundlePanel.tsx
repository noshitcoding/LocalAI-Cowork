import {useState} from "react";
import {RequestError,type Client,type Health,type AuditDiagnostics} from "@cowork/client";
import {deliverDownload} from "./download";
import {useView} from "./view";

export function SupportBundlePanel({client}:{client:Client}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b;
  const [bundle,setBundle]=useState<unknown>(),[busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  async function collect(){setBusy(true);setError("");setNotice("");try{
    const result=await Promise.allSettled([client.health(),client.one("/audit/diagnostics","AuditDiagnostics")]);
    const section=<T,>(item:PromiseSettledResult<T>)=>item.status==="fulfilled"?{status:"available",data:item.value}:{status:"unavailable",code:item.reason instanceof RequestError?item.reason.details.code:"diagnostics_unavailable"};
    setBundle({format:"cowork.support.bundle/1",captured_at:Date.now(),redaction:"No chat content, credentials, local paths, command arguments or model requests are collected.",health:section(result[0] as PromiseSettledResult<Health>),audit:section(result[1] as PromiseSettledResult<AuditDiagnostics>)});
  }catch{setError(tr("Diagnosedaten konnten nicht gesammelt werden.","Diagnostics could not be collected."));}finally{setBusy(false);}}
  async function save(){setBusy(true);setError("");try{const status=await deliverDownload(new Blob([JSON.stringify(bundle,null,2)+"\n"],{type:"application/json"}),"cowork-support-bundle.json",language);setNotice(status==="saved"?tr("Supportbundle gespeichert.","Support bundle saved."):status==="canceled"?tr("Speichern abgebrochen.","Save canceled."):tr("Download bereitgestellt.","Download prepared."));}catch{setError(tr("Supportbundle konnte nicht gespeichert werden.","Support bundle could not be saved."));}finally{setBusy(false);}}
  return <section className="settings-card"><h2>{tr("Supportbundle","Support bundle")}</h2><p>{tr("Sammle Versions-, Laufzeit- und redigierte Audit-Metadaten für die Diagnose. Die Datei wird auf diesem Gerät gespeichert.","Collect version, runtime and redacted audit metadata for diagnostics. The file is saved on this device.")}</p><div className="actions"><button type="button" disabled={busy} onClick={()=>void collect()}>{tr("Diagnosedaten sammeln","Collect diagnostics")}</button>{bundle!==undefined&&<button type="button" disabled={busy} onClick={()=>void save()}>{tr("Supportbundle herunterladen","Download support bundle")}</button>}</div>{error&&<p role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}{bundle!==undefined&&<details><summary>{tr("Enthaltene Metadaten ansehen","Inspect included metadata")}</summary><pre>{JSON.stringify(bundle,null,2)}</pre></details>}</section>;
}
