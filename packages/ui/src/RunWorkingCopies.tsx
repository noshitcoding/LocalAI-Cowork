import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {ArrowUp,Download,FileText,Folder,RefreshCw} from "lucide-react";
import {Client,RequestError,type RunRecord,type WorkspaceSnapshot} from "@cowork/client";
import {deliverDownload,deliveryStatusKey} from "./download";
import {useView} from "./view";
import {type Key} from "./i18n";

export function RunWorkingCopies({client,order,run,snapshot,t}:{client:Client;order:string;run:RunRecord;snapshot?:WorkspaceSnapshot|null;t:(key:Key)=>string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const managed=!!run.spec.environment&&!run.spec.workflow;
  const folders=run.spec.folders??[];
  const [folder,setFolder]=useState(folders[0]?.id??""),[path,setPath]=useState("."),[selected,setSelected]=useState("");
  const [downloading,setDownloading]=useState(false),[message,setMessage]=useState<Key|null>(null),[downloadError,setDownloadError]=useState(false);
  const listing=useQuery({queryKey:["run-copy-files",order,run.id,folder,path],queryFn:()=>client.runCopyFiles(order,run.id,{folder_id:folder,path}),enabled:!!folder,retry:false,refetchInterval:3000});
  const entry=listing.data?.entries.find(e=>e.path===selected);
  const previewable=listing.isSuccess&&entry?.kind==="file"&&entry.size<=1_000_000;
  const preview=useQuery({queryKey:["run-copy-text",order,run.id,folder,selected],queryFn:()=>client.runCopyText(order,run.id,{folder_id:folder,path:selected}),enabled:!!previewable,retry:false,refetchInterval:3000});
  const busy=(error:unknown)=>error instanceof RequestError&&["workspace_busy","transfer_busy"].includes(error.details.code);
  function navigate(next:string){setPath(next);setSelected("");}
  async function download(path:string,sha:string){if(downloading||!snapshot)return;setDownloading(true);setDownloadError(false);setMessage(null);
    try{const result=await client.downloadWorkspace(order,{path,expected_sha256:sha},snapshot.run_id);setMessage(deliveryStatusKey(await deliverDownload(result.blob,result.file_name,language,result)));}
    catch{setDownloadError(true);}finally{setDownloading(false);}
  }
  return <section className="working-files" aria-label={tr("Arbeitskopie dieses Laufs","This run's working copy")}>
    <header><h3>{tr("Arbeitskopie dieses Laufs","This run's working copy")}</h3><button className="icon" aria-label={t("refreshFiles")} disabled={listing.isFetching} onClick={()=>{void listing.refetch();if(previewable)void preview.refetch();}}><RefreshCw size={17}/></button></header>
    <p className="muted">{managed?tr("Diese Arbeitskopie und das getrennte Runtime-Ergebnis werden unabhängig aufbewahrt.","This working copy and the separate runtime result are retained independently."):tr("Diese Dateien gehören zum gewählten Lauf. Das abgeschlossene Ergebnis bleibt als eigene Version erhalten.","These files belong to the selected run. Its completed result remains a separate version.")}</p>
    {folders.length>0?<label>{tr("Ordner in der Arbeitskopie","Working copy folder")}<select value={folder} onChange={e=>{setFolder(e.target.value);navigate(".");}}>{folders.map(f=><option value={f.id} key={f.id}>{f.name}</option>)}</select></label>:<p>{tr("Dieser Lauf hat keine Dateiordner.","This run has no file folders.")}</p>}
    {folder&&<><div className="working-path"><code>{path}</code>{path!=="."&&<button className="quiet" onClick={()=>navigate(path.includes("/")?path.slice(0,path.lastIndexOf("/")):".")}><ArrowUp size={16}/>{t("folderUp")}</button>}</div>
      {listing.isPending&&<p role="status">{t("loading")}</p>}
      {listing.isError&&(busy(listing.error)?<p role="status">{t("workingFilesBusy")}</p>:<p role="alert">{tr("Die Arbeitskopie konnte nicht gelesen werden.","The working copy could not be read.")}{snapshot&&" "+tr("Die gespeicherte Ergebnisversion findest du unten.","Its retained result version is listed below.")}</p>)}
      {listing.isSuccess&&!listing.data.entries.length&&<p>{t("workingFilesEmpty")}</p>}
      <ul className="working-file-list">{listing.data?.entries.map(e=><li key={e.path}><button className="quiet" disabled={!["file","directory"].includes(e.kind)} aria-pressed={e.kind==="file"?selected===e.path:undefined} onClick={()=>e.kind==="directory"?navigate(e.path):setSelected(e.path)}>{e.kind==="directory"?<Folder size={17}/>:<FileText size={17}/>}<span>{e.name}</span><small>{e.kind==="file"?`${e.size} B`:""}</small></button></li>)}</ul>
      {listing.data?.truncated&&<p>{t("workingListTruncated")}</p>}
      {selected&&<section className="working-preview"><h4>{selected}</h4>{previewable&&preview.data?<><pre>{preview.data.content}</pre><details><summary>{t("artifactIntegrity")}</summary><code>{preview.data.sha256}</code></details></>:<p role="status">{t(preview.isPending&&previewable?"loading":"workspacePreviewUnavailable")}</p>}</section>}
    </>}
    {message&&<p role="status">{t(message)}</p>}{downloading&&<p role="status">{t("downloadChecking")}</p>}{downloadError&&<p role="alert">{t("downloadFailed")}</p>}
    {snapshot&&<details className="workspace-snapshot" open><summary>{managed?tr("Gespeicherte Arbeitskopie","Retained working copy"):tr("Gespeicherte Ergebnisversion","Retained result version")} · {snapshot.entries.length}</summary><p>{t("workspaceSnapshotHint")}</p><ul>{snapshot.entries.map(e=><li key={e.path}><span>{e.path}{e.sha256?` · ${e.size} B`:"/"}</span>{e.sha256&&<button className="icon file-download" disabled={downloading} aria-label={t("downloadSnapshot")+": "+e.path} onClick={()=>void download(e.path,e.sha256!)}><Download size={17}/></button>}</li>)}</ul><details><summary>{t("artifactIntegrity")}</summary><code>{snapshot.sha256}</code></details></details>}
  </section>;
}
