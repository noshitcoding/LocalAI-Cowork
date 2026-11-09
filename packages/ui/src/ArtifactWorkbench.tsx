import {useEffect,useId,useState} from "react";
import type {Artifact,DownloadedFile} from "@cowork/client";
import {Download} from "lucide-react";
import {artifactDocument,artifactVersions,documentKind} from "./documentFiles";
import {DocumentPreview} from "./DocumentPreview";
import {deliverDownload} from "./download";
import {useView} from "./view";
import "./documentWorkbench.css";

export function ArtifactWorkbench({artifact,artifacts,onSelect}:{artifact:Artifact,artifacts:Artifact[],onSelect:(artifact:Artifact)=>void}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [tab,setTab]=useState<"preview"|"source"|"versions">("preview"),[file,setFile]=useState<DownloadedFile|null>(null),[error,setError]=useState(""),[busy,setBusy]=useState(false),[notice,setNotice]=useState("");
  const currentFile=file?.sha256===artifact.sha256&&file.file_name===artifact.name?file:null;
  const versions=artifactVersions(artifact,artifacts),kind=documentKind(artifact.name,artifact.media_type),hasSource=["html","markdown","text"].includes(kind);
  const displayedTab=tab==="source"&&!hasSource?"preview":tab,prefix=useId(),tabs=(hasSource?["preview","source","versions"]:["preview","versions"]) as (typeof tab)[];
  function navigate(event:React.KeyboardEvent<HTMLButtonElement>,index:number){const next=event.key==="ArrowRight"?(index+1)%tabs.length:event.key==="ArrowLeft"?(index+tabs.length-1)%tabs.length:event.key==="Home"?0:event.key==="End"?tabs.length-1:-1;if(next<0)return;event.preventDefault();setTab(tabs[next]);event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();}
  useEffect(()=>{let live=true;setFile(null);setError("");setNotice("");void artifactDocument(artifact).then(file=>{if(live)setFile(file);},error=>{if(live)setError(error instanceof Error?error.message:String(error));});return()=>{live=false;};},[artifact.id,artifact.sha256,artifact.content,artifact.name,artifact.media_type,artifact.size]);
  async function save(){if(!currentFile||busy)return;setBusy(true);setError("");try{const status=await deliverDownload(currentFile.blob,currentFile.file_name,language,currentFile);setNotice(status==="saved"?tr("Datei gespeichert.","File saved."):status==="canceled"?tr("Speichern abgebrochen.","Save canceled."):tr("Download bereitgestellt.","Download prepared."));}catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}}
  return <section className="document-workbench" aria-label={tr("Dokumentansicht","Document workbench")}><header><div><h3>{artifact.name}</h3><small>{artifact.media_type} · {artifact.size.toLocaleString(language)} B</small></div><button type="button" className="quiet" disabled={!currentFile||busy} onClick={()=>void save()}><Download size={16}/>{tr("Herunterladen","Download")}</button></header>
    <div className="document-tabs" role="tablist" aria-label={tr("Dokumentansicht","Document view")}>{tabs.map((value,index)=><button type="button" key={value} id={prefix+value} role="tab" aria-selected={displayedTab===value} aria-controls={prefix+"-panel"} tabIndex={displayedTab===value?0:-1} onKeyDown={event=>navigate(event,index)} onClick={()=>setTab(value)}>{value==="preview"?tr("Vorschau","Preview"):value==="source"?tr("Quelltext","Source"):tr("Versionen","Versions")}</button>)}</div>
    {error&&<p className="error" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
    <div role="tabpanel" id={prefix+"-panel"} aria-labelledby={prefix+displayedTab}>{displayedTab==="versions"?<><p className="muted">{tr("Gespeicherte Ergebnisse mit demselben Dateinamen in diesem Chat. Jede Ausgabe bleibt unveränderlich mit ihrem Lauf und Hash verknüpft.","Saved outputs with the same file name in this chat. Each output remains immutable and bound to its run and hash.")}</p><ul className="document-version-list">{versions.map(version=><li key={version.id}><button type="button" aria-pressed={version.id===artifact.id} onClick={()=>onSelect(version)}><span>{new Date(version.created_at).toLocaleString(language)} · {version.size.toLocaleString(language)} B</span><small>{tr("Lauf","Run")}: {version.run_id}</small><code>{version.sha256}</code></button></li>)}</ul></>:currentFile?<DocumentPreview key={artifact.id} file={currentFile} source={displayedTab==="source"}/>:!error&&<p role="status">{tr("Dokument wird geprüft…","Verifying document…")}</p>}</div>
    <details className="document-integrity"><summary>{tr("Integrität & Herkunft","Integrity & origin")}</summary><dl><dt>SHA256</dt><dd><code>{artifact.sha256}</code></dd><dt>{tr("Artefakt","Artifact")}</dt><dd><code>{artifact.id}</code></dd><dt>{tr("Lauf","Run")}</dt><dd><code>{artifact.run_id}</code></dd><dt>{tr("Erstellt","Created")}</dt><dd>{new Date(artifact.created_at).toLocaleString(language)}</dd></dl></details>
  </section>;
}
