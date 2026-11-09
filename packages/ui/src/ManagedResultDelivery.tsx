import {useEffect,useRef,useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {RequestError,type Client,type OwnerManagedResultEntry} from "@cowork/client";
import {useView} from "./view";

export function ManagedResultDelivery({client,connection,entry,available,historyOnly=false}:{client:Client;connection:number;entry:OwnerManagedResultEntry;available:boolean;historyOnly?:boolean}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const version=entry.version;
  const current=useRef(true),pending=useRef(false);
  const [working,setWorking]=useState(false),[failed,setFailed]=useState(false);
  useEffect(()=>{current.current=true;return()=>{current.current=false;};},[]);
  const history=useQuery({queryKey:["owner-managed-result-delivery",connection,entry.order_id,entry.run_id,entry.process_attempt_id,version?.id??"history"],
    queryFn:()=>client.ownerManagedResultDelivery(entry.order_id,entry.run_id,entry.process_attempt_id),retry:false,refetchOnWindowFocus:false});
  const fresh=history.isSuccess&&!history.isFetching;
  const value=fresh?history.data?.delivery:null;
  const mismatch=!!value&&!!version&&(value.result_version_id!==version.id||value.manifest_sha256!==version.manifest_sha256);
  const usable=fresh&&!mismatch;
  async function act(action:"prepare"|"confirm"|"refresh") {
    if(pending.current||!usable)return;
    if(action!=="refresh"&&(!available||historyOnly||!version))return;
    if(action==="confirm"&&(!value||!["prepared","committed"].includes(value.state)))return;
    if(action==="prepare"&&value&&value.state!=="expired")return;
    if(action==="refresh"&&(!value||!["committed","accepted","unconfirmed"].includes(value.state)))return;
    pending.current=true;setWorking(true);setFailed(false);
    try {
      const args=[entry.order_id,entry.run_id,entry.process_attempt_id] as const;
      if(action==="prepare")await client.prepareOwnerManagedResultDelivery(...args);
      else if(action==="confirm")await client.confirmOwnerManagedResultDelivery(...args,{id:value!.id,request_sha256:value!.request_sha256});
      else await client.refreshOwnerManagedResultRetirement(...args);
    } catch {if(current.current)setFailed(true);}
    finally {
      // This is a local history GET only. It never repeats either mutation.
      if(current.current){await history.refetch();if(current.current)setWorking(false);}
      pending.current=false;
    }
  }
  const disabled=working||history.isFetching;
  if(historyOnly&&((fresh&&!value)||(history.isError&&history.error instanceof RequestError&&history.error.status===404)))return null;
  return <section aria-label={tr("Runner-Speicherfreigabe","Runner storage release")}>
    <h5>{tr("Runner-Speicherfreigabe","Runner storage release")}</h5>
    <p>{tr("Bestätige nach dem geprüften lokalen Empfang, dass der Runner seine Ausgabekopie freigeben darf. Deine lokale Ergebnisversion bleibt erhalten.","After verified local receipt, confirm that the runner may release its output copy. Your local result version is retained.")}</p>
    {(history.isPending||history.isFetching||working)&&<p role="status">{tr("Bestätigungsstand wird geprüft …","Checking confirmation history …")}</p>}
    {(history.isError||mismatch)&&<p role="alert">{tr("Der Bestätigungsstand konnte nicht geprüft werden.","Confirmation history could not be verified.")} <button disabled={disabled} onClick={()=>void history.refetch()}>{tr("Erneut prüfen","Check again")}</button></p>}
    {failed&&<p role="alert">{tr("Die Antwort wurde nicht bestätigt. Prüfe den gespeicherten Stand. Eine Wiederholung sendet ausschließlich die ursprüngliche Bestätigung; eine neue Bestätigung ist erst nach nachgewiesenem Ablauf möglich.","The reply was not confirmed. Check the retained history. A retry sends only the original confirmation; a new confirmation requires confirmed expiry.")}</p>}
    {usable&&!historyOnly&&(!value||value.state==="expired")&&<>
      {value&&<p>{tr("Die ursprüngliche Bestätigung wurde vor Ablauf nicht angenommen.","The original confirmation was not accepted before expiry.")}</p>}
      <button disabled={disabled||!available} onClick={()=>void act("prepare")}>{tr("Speicherfreigabe vorbereiten","Prepare storage release")}</button>
    </>}
    {usable&&value&&["prepared","committed"].includes(value.state)&&<>
      <p>{value.state==="committed"?tr("Bestätigung gesendet; Annahme noch nicht nachgewiesen.","Confirmation sent; acceptance has not been established."):tr("Bestätigung vorbereitet; noch nicht an den Runner gesendet.","Confirmation prepared; not yet sent to the runner.")}</p>
      <p>{tr("Lokale Version","Local version")}: <code>{value.result_version_id}</code>{version&&<> · {version.file_count} {tr("Dateien","files")} · {version.total_bytes} B</>}</p>
      <details><summary>{tr("Bestätigung prüfen","Review confirmation")}</summary><p>{tr("Anfrage","Request")}: <code>{value.request_sha256}</code></p><p>{tr("Manifest","Manifest")}: <code>{value.manifest_sha256}</code></p><p>{tr("Erstannahme bis","First acceptance before")}: {new Date(value.accept_before).toISOString()}</p></details>
      {value.state==="committed"&&<button disabled={disabled} onClick={()=>void act("refresh")}>{tr("Ursprüngliche Bestätigungsquittung prüfen","Check original confirmation receipt")}</button>}
      <button disabled={disabled||!available||historyOnly} onClick={()=>void act("confirm")}>{value.state==="committed"?tr("Ursprüngliche Bestätigung erneut abgleichen","Reconcile original confirmation"):tr("Empfang bestätigen und Runner-Speicher freigeben","Confirm receipt and release runner storage")}</button>
    </>}
    {usable&&value&&["accepted","unconfirmed","retired"].includes(value.state)&&<>
      <p>{tr("Empfang vom Runner bestätigt.","Receipt acknowledged by the runner.")}</p>
      <p role="status">{value.state==="retired"?tr("Physische Speicherfreigabe nachgewiesen.","Physical storage release verified."):value.state==="unconfirmed"?tr("Physische Speicherfreigabe derzeit nicht nachgewiesen.","Physical storage release is currently unconfirmed."):tr("Physische Speicherfreigabe steht noch aus.","Physical storage release is pending.")}</p>
      {value.state!=="retired"&&<button disabled={disabled} onClick={()=>void act("refresh")}>{tr("Freigabestatus prüfen","Check release status")}</button>}
      <details><summary>{tr("Quittung prüfen","Review receipt")}</summary><code>{value.delivery_receipt_sha256}</code>{value.retirement_receipt_sha256&&<p><code>{value.retirement_receipt_sha256}</code></p>}</details>
    </>}
  </section>;
}
