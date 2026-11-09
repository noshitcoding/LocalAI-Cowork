import {useState} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {RequestError,type NativeChooseFolderRequest,type RegisteredFolder,type UiLanguage} from "@cowork/client";
import {hasNativeFolderManager,listNativeFolders,chooseNativeFolder,revokeNativeFolder} from "./folderBridge";
import {de,type Key} from "./i18n";

export function NativeFolders({project,language,t}:{project:string,language:UiLanguage,t:(key:Key)=>string}) {
  const native=hasNativeFolderManager();const qc=useQueryClient();
  const records=useQuery({queryKey:["native-folders",project],queryFn:()=>listNativeFolders(project),enabled:native,retry:false});
  const [writable,setWritable]=useState(false);const [busy,setBusy]=useState(false);
  const [pending,setPending]=useState<NativeChooseFolderRequest|null>(null);const [error,setError]=useState<unknown>();
  const [status,setStatus]=useState<Key|null>(null);const [removing,setRemoving]=useState<RegisteredFolder|null>(null);
  const refresh=async()=>{await Promise.all([qc.invalidateQueries({queryKey:["native-folders",project]}),qc.invalidateQueries({queryKey:["folders",project]})]);};
  async function choose(){
    if(busy)return;setBusy(true);setError(undefined);setStatus(null);
    const input=pending??{project_id:project,idempotency_key:crypto.randomUUID(),writable,language};setPending(input);
    try {const result=await chooseNativeFolder(input);setPending(null);setStatus(result.status==="canceled"?"folderChoiceCanceled":result.folder.revoked?"folderRevoked":"folderRegistered");await refresh();}
    catch(error){setError(error);if(error instanceof RequestError&&error.details.code!=="folder_registration_uncertain")setPending(null);}
    finally{setBusy(false);}
  }
  async function revoke(){
    if(!removing||busy)return;setBusy(true);setError(undefined);setStatus(null);
    try {await revokeNativeFolder({project_id:project,folder_id:removing.grant.id,expected_revision:removing.grant.revision});setRemoving(null);setStatus("folderRevoked");await refresh();}
    catch(error){setError(error);}finally{setBusy(false);}
  }
  const shownError=error??records.error;const code=shownError instanceof RequestError?shownError.details.code:"folder_connection_failed";
  return <section className="native-folders"><h3>{t("manageFolderAccess")}</h3>
    {!native?<p>{t("folder_desktop_required")}</p>:<>
      <p>{t("nativeFolderHint")}</p>
      <label className="replace-choice"><input type="checkbox" checked={writable} disabled={busy||!!pending} onChange={e=>setWritable(e.target.checked)}/>{t("grantFolderChanges")}</label>
      <div className="actions"><button disabled={busy} onClick={()=>void choose()}>{t(pending?"retryFolderChoice":"chooseFolder")}</button><button disabled={busy} onClick={()=>void refresh()}>{t("refreshFolderAccess")}</button></div>
      {pending&&<p>{t("folderChoicePending")}</p>}{status&&<p role="status">{t(status)}</p>}
      {shownError&&<p role="alert">{t(code==="workspace_busy"?"folderRevokeBusy":code in de?code as Key:"folder_connection_failed")}</p>}
      {records.isPending&&<p role="status">{t("loading")}</p>}
      <ul className="native-folder-list">{records.data?.map(record=><li key={record.grant.id}><strong>{record.grant.name}</strong><span className="native-folder-path">{record.canonical_path}</span><small>{t(record.revoked?"folderAccessRevoked":record.grant.writable?"folderWritable":"folderReadOnly")}</small>
        {!record.revoked&&<button disabled={busy} onClick={()=>{setRemoving(record);setError(undefined);}}>{t("revokeFolderAccess")}</button>}
      </li>)}</ul>
      {removing&&<section className="folder-revoke-confirm"><h4>{t("revokeFolderAccess")}</h4><p>{removing.canonical_path}</p><p>{t("folderRevokeHint")}</p><div className="actions"><button disabled={busy} onClick={()=>setRemoving(null)}>{t("cancel")}</button><button disabled={busy} onClick={()=>void revoke()}>{t("confirmFolderRevocation")}</button></div></section>}
    </>}
  </section>;
}
