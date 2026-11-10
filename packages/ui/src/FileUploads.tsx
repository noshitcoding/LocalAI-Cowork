import {useRef,useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {Client,RequestError,uploadMetadata,type FileUpload,type ReserveUpload,type FileStat,type ImportFile,type AccessMode} from "@cowork/client";
import {de,type Key} from "./i18n";
import {safeFileName} from "./download";
type T=(key:Key)=>string;

function ErrorText({error,t}:{error:unknown,t:T}) {
  if(!error)return null;
  const code=error instanceof RequestError?error.details.code:error instanceof Error?error.message:"";
  return <p role="alert">{t(code in de?code as Key:"uploadFailed")}</p>;
}

export function FileUploads({order,client,t,prepareImport}:{order:string,client:Client,t:T,prepareImport:(upload:FileUpload)=>void}) {
  const qc=useQueryClient();const [offset,setOffset]=useState(0);
  const uploads=useQuery({queryKey:["uploads",order,offset],queryFn:()=>client.uploads(order,offset)});
  const [file,setFile]=useState<File|null>(null);const [reservation,setReservation]=useState<FileUpload|null>(null);
  const request=useRef<ReserveUpload|null>(null);const chooser=useRef<HTMLInputElement>(null);
  const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();const [ready,setReady]=useState(false);
  async function transfer(e:FormEvent){
    e.preventDefault();if(!file||busy)return;setBusy(true);setError(undefined);setReady(false);
    try {
      const metadata=await uploadMetadata(file,file.name);
      let current=reservation;
      if(current){if(current.file.size!==metadata.size||current.file.sha256!==metadata.sha256)throw new Error("uploadMismatch");}
      else {
        request.current??={idempotency_key:crypto.randomUUID(),file:metadata};
        current=await client.reserveUpload(order,request.current);setReservation(current);
      }
      const complete=await client.uploadContent(order,current,file);
      setReservation(complete);setFile(null);if(chooser.current)chooser.current.value="";request.current=null;setReady(true);
      await qc.invalidateQueries({queryKey:["uploads",order]});
    }catch(error){setError(error);await qc.invalidateQueries({queryKey:["uploads",order]});}
    finally{setBusy(false);}
  }
  function reset(){setReservation(null);setFile(null);request.current=null;setError(undefined);setReady(false);if(chooser.current)chooser.current.value="";}
  return <details className="file-uploads"><summary>{t("addFile")}</summary>
    <p className="muted">{t("uploadHint")}</p><button type="button" disabled={busy} onClick={()=>void uploads.refetch()}>{t("refreshUploads")}</button>
    <form onSubmit={transfer}>
      {reservation?.state==="reserved"&&<p>{t("uploadContinue")}: <strong>{reservation.file.file_name}</strong></p>}
      <label>{t("chooseLocalFile")}<input ref={chooser} type="file" disabled={busy} onChange={e=>{setFile(e.target.files?.[0]??null);setError(undefined);setReady(false);if(!reservation||reservation.state==="ready"){request.current=null;setReservation(null);}}}/></label>
      {file&&<p>{file.name} · {new Intl.NumberFormat().format(file.size)} B</p>}
      {busy&&<p role="status">{t("uploadBusy")}</p>}{ready&&<p role="status">{t("uploadReadyHint")}</p>}<ErrorText error={error} t={t}/>
      <div className="actions"><button type="button" disabled={busy} onClick={reset}>{t("clearSelection")}</button><button disabled={busy||!file}>{t("uploadFile")}</button></div>
    </form>
    {uploads.isError&&<ErrorText error={uploads.error} t={t}/>}
    {!!uploads.data?.items.length&&<><h4>{t("uploadedFiles")}</h4><ul className="upload-list">{uploads.data.items.map(upload=><li key={upload.id}><span>{upload.file.file_name}<small>{new Intl.NumberFormat().format(upload.file.size)} B · {t(upload.state==="ready"?"uploadReady":"uploadReserved")}</small></span>
      {upload.state==="ready"?<button disabled={busy} onClick={()=>prepareImport(upload)}>{t("import_file")}</button>:<button disabled={busy} onClick={()=>{reset();setReservation(upload);}}>{t("uploadContinue")}</button>}
    </li>)}</ul><div className="actions"><button disabled={busy||offset===0} onClick={()=>setOffset(Math.max(0,offset-30))}>{t("previousPage")}</button><button disabled={busy||uploads.data.next_offset==null} onClick={()=>setOffset(uploads.data!.next_offset!)}>{t("nextPage")}</button></div></>}
  </details>;
}

export function ImportFileForm({upload,client,t,mode,setMode,busy,submit}:{upload:FileUpload,client:Client,t:T,mode:AccessMode,setMode:(mode:AccessMode)=>void,busy:boolean,submit:(input:ImportFile)=>Promise<void>}) {
  const [path,setPath]=useState(safeFileName(upload.file.file_name));const [observed,setObserved]=useState<FileStat|null>(null);
  const [replace,setReplace]=useState(false);const [checking,setChecking]=useState(false);const [error,setError]=useState<unknown>();
  async function check(){setChecking(true);setError(undefined);setObserved(null);try{setObserved(await client.workspaceStat(upload.order_id,path));}catch(error){setError(error);}finally{setChecking(false);}}
  async function send(e:FormEvent){e.preventDefault();if(!observed||mode==="read_only"||observed.node&&(!replace||observed.node.kind!=="file"))return;
    await submit({folder_id:observed.folder_id,path:observed.path,upload_id:upload.id,file:upload.file,expected_sha256:observed.node?.sha256??null});
    setObserved(null);setReplace(false);}
  return <form onSubmit={send}><p>{t("importHint")}</p><p><strong>{upload.file.file_name}</strong> · {new Intl.NumberFormat().format(upload.file.size)} B</p>
    <label>{t("destinationPath")}<input value={path} required maxLength={4096} disabled={busy||checking} onChange={e=>{setPath(e.target.value);setObserved(null);setReplace(false);}}/></label>
    <label>{t("access")}<select value={mode} disabled={busy} onChange={e=>setMode(e.target.value as AccessMode)}>{(["read_only","confirm_mutations","confirm_all","autonomous"] as const).map(m=><option key={m} value={m}>{t(m)}</option>)}</select></label>
    {mode==="read_only"&&<p role="alert">{t("importReadOnly")}</p>}
    <button type="button" disabled={busy||!path.trim()} aria-disabled={checking} onClick={()=>{if(!checking)void check();}}>{t("checkDestination")}</button>
    {checking&&<p role="status">{t("loading")}</p>}<ErrorText error={error} t={t}/>
    {observed&&<><p>{t(observed.node?"destinationExists":"destinationNew")}</p>{observed.node?.kind==="directory"?<p role="alert">{t("destinationDirectory")}</p>:observed.node&&<label className="replace-choice"><input type="checkbox" checked={replace} disabled={busy} onChange={e=>setReplace(e.target.checked)}/>{t("confirmReplace")}</label>}
      <details><summary>{t("artifactIntegrity")}</summary><dl><dt>{t("previousVersion")}</dt><dd><code>{observed.node?.sha256??"—"}</code></dd><dt>{t("proposedVersion")}</dt><dd><code>{upload.file.sha256}</code></dd></dl></details></>}
    <div className="actions"><button className="primary" disabled={busy||checking||mode==="read_only"||!observed||!!observed.node&&(!replace||observed.node.kind!=="file")}>{t("startImport")}</button></div>
  </form>;
}
