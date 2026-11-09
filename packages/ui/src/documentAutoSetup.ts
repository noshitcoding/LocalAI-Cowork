import {useEffect,useSyncExternalStore} from "react";
import {useQueryClient,type QueryCache} from "@tanstack/react-query";

// Mirrors apps/desktop/src-tauri/src/tool_setup.rs (SetupStatus, AutoSetup).
export type SetupProfile="documents"|"full";
export type AutoSetup={attempts:number;last_attempt_at:number|null;next_attempt_at:number|null;last_error:string|null;paused_by_user:boolean};
export type SetupStatus={running:boolean;stage:string;message:string;ready:boolean;programs:Record<string,string>;error:string|null;profile?:SetupProfile|null;automatic?:boolean;documents_ready?:boolean;browser_ready?:boolean;auto?:AutoSetup|null};
// Fields of the daemon's /toolbox availability that belong to the document tools.
export type DocumentAvailability={documents_available?:boolean;latex_available?:boolean;office_convert_available?:boolean;document_status?:string;latex_status?:string;office_convert_status?:string;ocr_available?:boolean;ocr_status?:string};

export type DocumentToolsState="unavailable"|"installing"|"ready"|"paused"|"failed"|"waiting"|"missing"|"path_too_long";
export type LatexState="active"|"installed_inactive"|"installing"|"missing"|"probing"|"cache_incomplete"|"isolation_unavailable"|"probe_failed"|"data_path_too_long";

export const isNativeDesktop=()=>typeof window!=="undefined"&&"__TAURI_INTERNALS__" in window;

/** Every setup profile installs the document runtime, so any running setup counts as installing. */
export function documentToolsState(status:SetupStatus|null,availability?:DocumentAvailability,now=Date.now()):DocumentToolsState{
  if(availability?.document_status==="data_path_too_long")return "path_too_long";
  if(!status)return "unavailable";
  if(status.running)return "installing";
  if(availability?.document_status==="probe_failed")return "failed";
  if(availability?.document_status==="probing")return "waiting";
  if(availability?.documents_available||status.documents_ready)return "ready";
  if(status.auto?.paused_by_user)return "paused";
  if(status.error||status.auto?.last_error)return "failed";
  if(status.auto?.next_attempt_at&&status.auto.next_attempt_at>now)return "waiting";
  return "missing";
}

/** Installed files are not evidence of a working cache or a working isolation boundary. */
export function latexState(status:SetupStatus|null,availability?:DocumentAvailability):LatexState{
  if(availability?.latex_status==="data_path_too_long")return "data_path_too_long";
  if(availability?.latex_available===true)return "active";
  if(status?.running)return "installing";
  const reason=availability?.latex_status;
  if(reason&&["probing","cache_incomplete","isolation_unavailable","probe_failed"].includes(reason))return reason as LatexState;
  if(status?.documents_ready&&status.programs.tectonic)return "installed_inactive";
  return "missing";
}

type Invoke=<T>(command:string,args?:Record<string,unknown>)=>Promise<T>;
const tauriInvoke:Invoke=async (command,args)=>{const {invoke}=await import("@tauri-apps/api/core");return invoke(command,args);};
let ensured:Promise<SetupStatus|null>|null=null;

/** Starts the automatic document setup at most once per app session; the desktop applies its own backoff. */
export function ensureDocumentTools(invoke:Invoke=tauriInvoke):Promise<SetupStatus|null>{
  if(!isNativeDesktop())return Promise.resolve(null);
  ensured??=invoke<SetupStatus>("toolbox_setup_ensure_documents").catch(()=>{ensured=null;return null;});
  return ensured;
}
export function resetDocumentAutoSetup(){ensured=null;}

/** Call once the native workspace is connected (after connection_info). */
export function useDocumentAutoSetup(connected:boolean){
  useEffect(()=>{if(connected)void ensureDocumentTools();},[connected]);
}

function latestToolbox(cache:QueryCache):DocumentAvailability|undefined{
  let latest:DocumentAvailability|undefined,at=-1;
  for(const query of cache.findAll({queryKey:["toolbox"]})){
    const {data,dataUpdatedAt}=query.state;
    if(data&&typeof data==="object"&&dataUpdatedAt>at){latest=data as DocumentAvailability;at=dataUpdatedAt;}
  }
  return latest;
}
/** The newest cached /toolbox availability of any workspace view, without issuing another request. */
export function useCachedToolboxAvailability():DocumentAvailability|undefined{
  const cache=useQueryClient().getQueryCache();
  return useSyncExternalStore(listener=>cache.subscribe(listener),()=>latestToolbox(cache),()=>undefined);
}
