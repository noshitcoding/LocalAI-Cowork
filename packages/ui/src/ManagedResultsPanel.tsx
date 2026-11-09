import {useEffect,useRef,useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {Download,RefreshCw} from "lucide-react";
import {Client,RequestError,type OwnerManagedResultEntry} from "@cowork/client";
import {deliverDownload,deliveryStatusKey} from "./download";
import {useView} from "./view";
import {type Key} from "./i18n";
import {ManagedResultDelivery} from "./ManagedResultDelivery";

const clients=new WeakMap<Client,{base:string;token:string;id:number}>();
let nextClient=1;
function clientScope(client:Client):number {
  const previous=clients.get(client);
  if(previous?.base===client.base&&previous.token===client.token)return previous.id;
  const id=nextClient++;clients.set(client,{base:client.base,token:client.token,id});return id;
}
function busy(error:unknown):boolean {
  return error instanceof RequestError&&["transfer_busy","workspace_busy"].includes(error.details.code);
}

type Props={client:Client;order:string;run:string;t:(key:Key)=>string};
export function ManagedResultsPanel(props:Props) {
  const connection=clientScope(props.client);
  return <ScopedResults key={JSON.stringify([connection,props.order,props.run])} {...props} connection={connection}/>;
}
function ScopedResults({client,order,run,t,connection}:Props&{connection:number}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  // Query keys use a local opaque connection identity, never a bearer token.
  const scope=JSON.stringify([connection,order,run]);
  const active=useRef(scope);active.current=scope;
  const pending=useRef(false),[receiving,setReceiving]=useState<string|null>(null),[failed,setFailed]=useState(false);
  const listing=useQuery({queryKey:["owner-managed-results",connection,order,run],queryFn:()=>client.ownerManagedResults(order,run),retry:false,refetchOnWindowFocus:false});
  // Original process IDs are history, not file grants. This fallback remains
  // independent of a denied content listing after cancellation/revocation.
  const processes=useQuery({queryKey:["owner-managed-result-process-history",connection,order,run],queryFn:async()=>{
    const detail=await client.detail(run);
    if(detail.run.id!==run||detail.run.spec.order_id!==order)throw new Error("managed_result_history_scope");
    const attempts=detail.process_attempts??[];
    if(attempts.length>128||attempts.some(attempt=>attempt.run_id!==run)||new Set(attempts.map(attempt=>attempt.id)).size!==attempts.length)throw new Error("managed_result_history_scope");
    return attempts.filter(attempt=>["completed","reconciled_completed"].includes(attempt.state.state));
  },enabled:listing.isError,retry:false,refetchOnWindowFocus:false});
  useEffect(()=>{active.current=scope;return()=>{active.current="";};},[scope]);
  async function receive(entry:OwnerManagedResultEntry) {
    if(pending.current||!listing.isSuccess||listing.isFetching)return;
    pending.current=true;const original=scope;setReceiving(entry.process_attempt_id);setFailed(false);
    try {await client.receiveOwnerManagedResult(order,run,entry.process_attempt_id);}
    catch {if(active.current===original)setFailed(true);}
    finally {
      if(active.current===original){await listing.refetch();setReceiving(null);}
      pending.current=false;
    }
  }
  const readable=listing.isSuccess&&!listing.isFetching;
  return <section className="working-files" aria-label={tr("Runtime-Ergebnisse","Runtime results")}>
    <header><h3>{tr("Runtime-Ergebnisse","Runtime results")}</h3><button className="icon" disabled={listing.isFetching||!!receiving} aria-label={tr("Runtime-Ergebnisse aktualisieren","Refresh runtime results")} onClick={()=>{setFailed(false);void listing.refetch();}}><RefreshCw size={17}/></button></header>
    <p className="muted">{tr("Rufe die Ausgaben des isolierten Prozesses ausdrücklich ab. Jede Ausgabe wird als eigene lokale Version geprüft und gespeichert.","Explicitly retrieve the isolated process outputs. Each output is verified and retained as its own local version.")}</p>
    {listing.isPending&&<p role="status">{t("loading")}</p>}
    {listing.isError&&<p role={busy(listing.error)?"status":"alert"}>{busy(listing.error)?t("workingFilesBusy"):tr("Die Ergebnisübersicht konnte nicht geprüft werden. Aktualisiere sie, um den Zugriff erneut zu prüfen.","The result overview could not be verified. Refresh it to recheck access.")}</p>}
    {receiving&&<p role="status">{tr("Ergebnis wird abgerufen und vollständig geprüft …","Retrieving and fully verifying the result …")}</p>}
    {failed&&<p role="alert">{tr("Der Abruf wurde nicht bestätigt. Der gespeicherte Stand wird erneut geprüft; eine Fortsetzung startest du ausdrücklich über den jeweiligen Abrufknopf.","Receipt was not confirmed. The retained state is checked again; use its retrieve button explicitly to continue.")}</p>}
    {readable&&!listing.data.entries.length&&<p>{tr("Für diesen Lauf liegt noch kein abgeschlossener Runtime-Prozessversuch vor.","This run has no completed runtime process attempt yet.")}</p>}
    {readable&&listing.data.entries.map((entry,index)=><article key={entry.process_attempt_id}>
      <h4>{tr("Prozessergebnis","Process result")} {index+1}</h4>
      {entry.version?.state==="ready"?<ReadyResult key={`${scope}:${entry.version.id}:${entry.version.revision}`} client={client} connection={connection} entry={entry} t={t}/>:<>
        <p>{entry.version?tr("Übertragung unvollständig","Transfer incomplete"):tr("Noch nicht lokal gespeichert","Not yet retained locally")}</p>
        <button disabled={!!receiving} onClick={()=>void receive(entry)}>{entry.version?tr("Fehlende Blöcke weiter abrufen","Continue retrieving missing blocks"):tr("Ergebnis abrufen","Retrieve result")}</button>
      </>}
    </article>)}
    {listing.isError&&processes.isError&&<p role="alert">{tr("Die ursprüngliche Prozesshistorie konnte nicht geprüft werden.","Original process history could not be verified.")} <button disabled={processes.isFetching} onClick={()=>void processes.refetch()}>{tr("Erneut prüfen","Check again")}</button></p>}
    {listing.isError&&processes.isSuccess&&!processes.isFetching&&Array.isArray(processes.data)&&processes.data.map(attempt=><ManagedResultDelivery
      key={`${scope}:history:${attempt.id}`} client={client} connection={connection} available={false} historyOnly
      entry={{order_id:order,run_id:run,process_attempt_id:attempt.id,version:null}}/>)}
  </section>;
}

function ReadyResult({client,connection,entry,t}:{client:Client;connection:number;entry:OwnerManagedResultEntry;t:(key:Key)=>string}) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const version=entry.version!;
  const [downloading,setDownloading]=useState(false),[message,setMessage]=useState<Key|null>(null),[failed,setFailed]=useState(false);
  const current=useRef(true),transfer=useRef<AbortController|null>(null);
  useEffect(()=>{current.current=true;return()=>{current.current=false;transfer.current?.abort();};},[]);
  const manifest=useQuery({queryKey:["owner-managed-result-manifest",connection,entry.order_id,entry.run_id,version.id,version.revision],
    queryFn:({signal})=>client.ownerManagedResultManifest(entry.order_id,entry.run_id,version.id,signal),retry:false,refetchOnWindowFocus:false});
  const available=manifest.isSuccess&&!manifest.isFetching&&manifest.data.entry.process_attempt_id===entry.process_attempt_id
    &&manifest.data.entry.version?.revision===version.revision&&manifest.data.manifest.sha256===version.manifest_sha256
    &&manifest.data.manifest.files.length===version.file_count&&manifest.data.manifest.total_bytes===version.total_bytes;
  const mismatch=manifest.isSuccess&&!manifest.isFetching&&!available;
  useEffect(()=>{if(manifest.isError||mismatch)transfer.current?.abort();},[manifest.isError,mismatch]);
  async function download(path:string) {
    if(!available||transfer.current)return;
    const abort=new AbortController();transfer.current=abort;setDownloading(true);setFailed(false);setMessage(null);
    try {
      const result=await client.downloadOwnerManagedResultFile(entry.order_id,entry.run_id,manifest.data!,path,abort.signal);
      if(!current.current||abort.signal.aborted)return;
      const status=await deliverDownload(result.blob,result.file_name,language,result,abort.signal);
      if(current.current)setMessage(deliveryStatusKey(status));
    } catch {if(current.current&&!abort.signal.aborted)setFailed(true);}
    finally {transfer.current=null;if(current.current)setDownloading(false);}
  }
  return <div>
    <p>{available?tr("Lokal gespeichert und vollständig geprüft","Retained locally and fully verified"):tr("Lokale Ergebnisversion","Local result version")} · {version.file_count} {tr("Dateien","files")} · {version.total_bytes} B</p>
    {(manifest.isPending||manifest.isFetching)&&<p role="status">{t("loading")}</p>}
    {(manifest.isError||mismatch)&&<p role="alert">{tr("Diese Version ist derzeit nicht lesbar. Prüfe den Zugriff erneut.","This version is currently unreadable. Recheck access.")} <button disabled={manifest.isFetching} onClick={()=>void manifest.refetch()}>{t("retry")}</button></p>}
    {downloading&&<p role="status">{t("downloadChecking")}</p>}{message&&<p role="status">{t(message)}</p>}{failed&&<p role="alert">{t("downloadFailed")}</p>}
    {available&&<ul className="working-file-list">{manifest.data.manifest.files.map(file=><li key={file.path}><span>{file.path} · {file.size} B</span><button className="icon" disabled={downloading} aria-label={tr("Ergebnisdatei herunterladen: ","Download result file: ")+file.path} onClick={()=>void download(file.path)}><Download size={17}/></button></li>)}</ul>}
    {available&&!manifest.data.manifest.files.length&&<p>{tr("Diese Version enthält keine Dateien.","This version contains no files.")}</p>}
    <details><summary>{t("artifactIntegrity")}</summary><code>{version.manifest_sha256}</code></details>
    <ManagedResultDelivery client={client} connection={connection} entry={entry} available={available}/>
  </div>;
}
