import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {Folder,FileText,ArrowUp,RefreshCw,Download} from "lucide-react";
import {Client,RequestError,type WorkspaceSnapshot,type FileUpload,type RunRecord} from "@cowork/client";
import {de,type Key} from "./i18n";
import {deliverDownload,deliveryStatusKey} from "./download";
import {useView} from "./view";
import {FileUploads} from "./FileUploads";
import {RunWorkingCopies} from "./RunWorkingCopies";
import {ManagedResultsPanel} from "./ManagedResultsPanel";

type WorkspaceRefresh = {isFetching:boolean;refetch:()=>Promise<{isSuccess:boolean}>};
export async function refreshWorkingFiles(workspace:WorkspaceRefresh,listing:()=>Promise<unknown>,preview?:()=>Promise<unknown>) {
  if(workspace.isFetching)return;
  const confirmed=await workspace.refetch();
  if(!confirmed.isSuccess)return;
  await listing();
  if(preview)await preview();
}

type Props={order:string,client:Client,t:(key:Key)=>string,snapshot?:WorkspaceSnapshot|null,run?:RunRecord|null,prepareImport:(upload:FileUpload)=>void,copyImport?:boolean};
export function WorkingFiles(props:Props) {
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const managed=!!props.run?.spec.environment&&!props.run?.spec.workflow;
  if(props.run?.spec.input_workspace)return <>
    <RunWorkingCopies key={props.run.id} order={props.order} client={props.client} t={props.t} run={props.run} snapshot={props.snapshot}/>
    {managed&&<ManagedResultsPanel order={props.order} client={props.client} run={props.run.id} t={props.t}/>}
    {props.copyImport&&<section className="working-files" aria-label={tr("Dateien für einen neuen Auftrag","Files for a new task")}>
      <h3>{tr("Dateien für einen neuen Auftrag","Files for a new task")}</h3>
      <p>{managed?tr("Ein Import beginnt einen neuen Auftrag mit einer eigenen bestätigten Arbeitskopie.","An import starts a new task with its own confirmed working copy."):tr("Ein Import erhält eine neue bestätigte Arbeitskopie. Um auf diesem Ergebnis aufzubauen, verzweige zuerst ab der zugehörigen Antwort.","An import receives a new confirmed working copy. To build on this result, first branch from its answer.")}</p>
      <FileUploads order={props.order} client={props.client} t={props.t} prepareImport={props.prepareImport}/>
    </section>}
  </>;
  return <ProjectWorkingFiles {...props}/>;
}
function ProjectWorkingFiles({order,client,t,snapshot,prepareImport}:Props) {
  const language=useView(v=>v.language);
  const [path,setPath]=useState(".");const [selected,setSelected]=useState("");
  const [downloading,setDownloading]=useState(false);const [downloadMessage,setDownloadMessage]=useState<Key|null>(null);const [downloadError,setDownloadError]=useState<Key|null>(null);
  const workspace=useQuery({queryKey:["workspace",order],queryFn:()=>client.workspace(order)});
  const workspaceReady=workspace.isSuccess&&!workspace.isFetching;
  const listing=useQuery({queryKey:["workspace-files",order,path],queryFn:()=>client.workspaceFiles(order,path),enabled:workspaceReady,refetchInterval:workspaceReady?5000:false});
  const selectedEntry=listing.data?.entries.find(entry=>entry.path===selected);
  const previewable=workspaceReady&&listing.isSuccess&&!!selectedEntry&&selectedEntry.kind==="file"&&selectedEntry.size<=1_000_000;
  const file=useQuery({queryKey:["workspace-text",order,selected],queryFn:()=>client.workspaceText(order,selected),enabled:previewable,retry:false,refetchInterval:query=>query.state.error?false:5000});
  const workspaceBusy=(error:unknown)=>error instanceof RequestError&&error.details.code==="workspace_busy";
  const temporarilyBusy=workspaceBusy(workspace.error)||workspaceBusy(listing.error);
  const filesFailed=workspace.isError&&!workspaceBusy(workspace.error)||listing.isError&&!workspaceBusy(listing.error);
  function refresh(){void refreshWorkingFiles(workspace,()=>listing.refetch(),previewable?()=>file.refetch():undefined);}
  function navigate(next:string){setPath(next);setSelected("");}
  async function download(path:string,run?:string,sha?:string|null){
    if(downloading)return;
    setDownloading(true);setDownloadError(null);setDownloadMessage(null);
    try {const result=await client.downloadWorkspace(order,{path,expected_sha256:sha},run);
      const status=await deliverDownload(result.blob,result.file_name,language,result);setDownloadMessage(deliveryStatusKey(status));
    } catch(error) {const code=error instanceof RequestError?error.details.code:"";setDownloadError(code==="file_conflict"?"downloadChanged":code==="workspace_busy"?"downloadBusy":code.startsWith("save_")&&code in de?code as Key:"downloadFailed");}
    finally {setDownloading(false);}
  }
  return <section className="working-files" aria-label={t("workingFiles")}>
    <header><h3>{t("workingFiles")}</h3><button className="icon" aria-label={t("refreshFiles")} disabled={workspace.isFetching} onClick={refresh}><RefreshCw size={17}/></button></header>
    <p className="muted">{t("workingFilesHint")}</p>
    <FileUploads order={order} client={client} t={t} prepareImport={prepareImport}/>
    {downloading&&<p role="status">{t("downloadChecking")}</p>}
    {downloadMessage&&<p role="status">{t(downloadMessage)}</p>}
    {downloadError&&<p role="alert">{t(downloadError)}</p>}
    {workspace.data?.base_snapshot_sha256&&<p className="muted">{t("workspaceInherited")}</p>}
    <div className="working-path"><code>{path}</code>{path!=="."&&<button className="quiet" onClick={()=>navigate(path.includes("/")?path.slice(0,path.lastIndexOf("/")):".")}><ArrowUp size={16}/>{t("folderUp")}</button>}</div>
    {(workspace.isFetching||workspaceReady&&listing.isPending)&&<p role="status">{t("loading")}</p>}
    {temporarilyBusy&&<p role="status">{t("workingFilesBusy")} <button disabled={workspace.isFetching} onClick={refresh}>{t("retry")}</button></p>}
    {filesFailed&&<p role="alert">{t("workingFilesUnavailable")} <button disabled={workspace.isFetching} onClick={refresh}>{t("retry")}</button></p>}
    {listing.data&&!listing.data.entries.length&&<p>{t("workingFilesEmpty")}</p>}
    <ul className="working-file-list">{listing.data?.entries.map(entry=><li key={entry.path}>
      <button className="quiet" disabled={!["file","directory"].includes(entry.kind)} aria-pressed={entry.kind==="file"?selected===entry.path:undefined} onClick={()=>entry.kind==="directory"?navigate(entry.path):setSelected(entry.path)}>
        {entry.kind==="directory"?<Folder size={17}/>:<FileText size={17}/>}<span>{entry.name}</span><small>{entry.kind==="file"?`${entry.size} B`:entry.kind!=="directory"?t("blockedFile"):""}</small>
      </button>{entry.kind==="file"&&<button className="icon file-download" aria-disabled={downloading||entry.size>67_108_864} aria-label={t("downloadCurrent")+": "+entry.path} title={entry.size>67_108_864?t("downloadLimit"):t("downloadCurrent")} onClick={()=>{if(entry.size<=67_108_864)void download(entry.path,undefined,file.data?.path===entry.path?file.data.sha256:undefined);}}><Download size={17}/></button>}</li>)}</ul>
    {listing.data?.truncated&&<p>{t("workingListTruncated")}</p>}
    {selected&&<section className="working-preview"><h4>{selected}</h4><p className="muted">{t("workspaceTextHint")}</p>{workspace.isFetching?<p role="status">{t("loading")}</p>:!previewable||file.isError?<p role="status">{t("workspacePreviewUnavailable")}</p>:file.data?<><pre>{file.data.content}</pre><details><summary>{t("artifactIntegrity")}</summary><code>{file.data.sha256}</code></details></>:<p role="status">{t("loading")}</p>}</section>}
    {snapshot&&<details className="workspace-snapshot"><summary>{t("workspaceSnapshot")} · {snapshot.entries.length}</summary><p>{t("workspaceSnapshotHint")}</p><ul>{snapshot.entries.map(entry=><li key={entry.path}><span>{entry.path}{entry.sha256?` · ${entry.size} B`:"/"}</span>{entry.sha256&&<button className="icon file-download" aria-disabled={downloading} aria-label={t("downloadSnapshot")+": "+entry.path} title={t("downloadSnapshot")} onClick={()=>void download(entry.path,snapshot.run_id,entry.sha256)}><Download size={17}/></button>}</li>)}</ul><details><summary>{t("artifactIntegrity")}</summary><code>{snapshot.sha256}</code></details></details>}
  </section>;
}
